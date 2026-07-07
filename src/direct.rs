use crate::runtime::{CdpClient, WebSocket, fetch_targets};
use crate::util::{read_json_file, write_atomic_0600};
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use reqwest::Method;
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, SET_COOKIE};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const GITCODE_HOST_SUFFIX: &str = "gitcode.com";
pub const REQUIRED_COOKIE_NAMES: [&str; 2] = ["GITCODE_ACCESS_TOKEN", "GITCODE_REFRESH_TOKEN"];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cookie {
  pub name: String,
  pub value: String,
  pub domain: String,
  pub path: String,
  pub expires: Option<f64>,
  #[serde(default)]
  pub secure: bool,
  #[serde(rename = "httpOnly", default)]
  pub http_only: bool,
  #[serde(rename = "sameSite")]
  pub same_site: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CookieAuth {
  pub cookies: Vec<Cookie>,
  #[serde(default)]
  pub source: String,
}

#[derive(Debug, Deserialize)]
struct AuthCache {
  cookies: Vec<Cookie>,
  #[serde(default)]
  source: String,
}

impl CookieAuth {
  pub fn from_cache(path: &Path) -> Result<Self> {
    let data: AuthCache = read_json_file(path)?;
    if data.cookies.is_empty() {
      bail!("auth cache has no cookies: {}", path.display());
    }
    Ok(Self {
      cookies: data
        .cookies
        .into_iter()
        .filter(|c| !c.name.is_empty() && !c.value.is_empty())
        .collect(),
      source: data.source,
    })
  }

  pub fn save(&self, path: &Path) -> Result<()> {
    let data = json!({
        "version": 1,
        "created": now(),
        "source": self.source,
        "cookies": self.cookies,
    });
    write_atomic_0600(
      path,
      (serde_json::to_string_pretty(&data)? + "\n").as_bytes(),
    )
  }

  pub fn valid(&self, margin: f64) -> bool {
    let available: HashSet<String> = self
      .cookies
      .iter()
      .filter(|cookie| is_gitcode_domain(&cookie.domain) && !cookie_expired(cookie, margin))
      .map(|cookie| cookie.name.clone())
      .collect();
    REQUIRED_COOKIE_NAMES
      .iter()
      .all(|name| available.contains(*name))
  }

  pub fn update(&mut self, cookies: Vec<Cookie>) {
    let mut by_key: HashMap<(String, String, String), Cookie> = HashMap::new();
    for cookie in self.cookies.drain(..) {
      by_key.insert(
        (
          cookie.domain.clone(),
          cookie.path.clone(),
          cookie.name.clone(),
        ),
        cookie,
      );
    }
    for cookie in cookies {
      if !is_gitcode_domain(&cookie.domain) {
        continue;
      }
      let key = (
        cookie.domain.clone(),
        cookie.path.clone(),
        cookie.name.clone(),
      );
      if cookie_expired(&cookie, 0.0) {
        by_key.remove(&key);
      } else {
        by_key.insert(key, cookie);
      }
    }
    self.cookies = by_key.into_values().collect();
  }

  pub fn cookie_header(&self, url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
      return String::new();
    };
    let host = parsed.host_str().unwrap_or("");
    let path = parsed.path();
    let mut selected: Vec<&Cookie> = self
      .cookies
      .iter()
      .filter(|cookie| {
        !cookie_expired(cookie, 0.0)
          && domain_matches(&cookie.domain, host)
          && path.starts_with(cookie.path.trim_end_matches('/'))
      })
      .collect();
    selected.sort_by_key(|cookie| std::cmp::Reverse(cookie.path.len()));
    selected
      .into_iter()
      .map(|cookie| format!("{}={}", cookie.name, cookie.value))
      .collect::<Vec<_>>()
      .join("; ")
  }

  pub fn xsrf_token(&self, url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
      return String::new();
    };
    let host = parsed.host_str().unwrap_or("");
    self
      .cookies
      .iter()
      .find(|cookie| cookie.name == "_xsrf" && domain_matches(&cookie.domain, host))
      .map(|cookie| cookie.value.clone())
      .unwrap_or_default()
  }

  pub fn redacted_names(&self) -> Vec<String> {
    let mut names: Vec<String> = self
      .cookies
      .iter()
      .filter(|cookie| is_gitcode_domain(&cookie.domain))
      .map(|cookie| cookie.name.clone())
      .collect();
    names.sort();
    names.dedup();
    names
  }
}

pub fn now() -> f64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|duration| duration.as_secs_f64())
    .unwrap_or(0.0)
}

pub fn is_gitcode_domain(domain: &str) -> bool {
  let clean = domain.trim_start_matches('.').to_ascii_lowercase();
  clean == GITCODE_HOST_SUFFIX || clean.ends_with(&format!(".{GITCODE_HOST_SUFFIX}"))
}

pub fn domain_matches(cookie_domain: &str, host: &str) -> bool {
  let domain = cookie_domain.to_ascii_lowercase();
  let host = host.to_ascii_lowercase();
  if let Some(clean) = domain.strip_prefix('.') {
    host == clean || host.ends_with(&format!(".{clean}"))
  } else {
    host == domain
  }
}

pub fn cookie_expired(cookie: &Cookie, margin: f64) -> bool {
  match cookie.expires {
    None => false,
    Some(expires) if expires < 0.0 => false,
    Some(expires) => expires <= now() + margin,
  }
}

fn normalize_cdp_cookie(value: &Value) -> Option<Cookie> {
  let name = value.get("name")?.as_str()?.to_string();
  let cookie_value = value.get("value")?.as_str()?.to_string();
  if name.is_empty() || cookie_value.is_empty() {
    return None;
  }
  Some(Cookie {
    name,
    value: cookie_value,
    domain: value
      .get("domain")
      .and_then(Value::as_str)
      .unwrap_or("")
      .to_string(),
    path: value
      .get("path")
      .and_then(Value::as_str)
      .unwrap_or("/")
      .to_string(),
    expires: value.get("expires").and_then(Value::as_f64),
    secure: value.get("secure").and_then(Value::as_bool).unwrap_or(true),
    http_only: value
      .get("httpOnly")
      .and_then(Value::as_bool)
      .unwrap_or(false),
    same_site: value
      .get("sameSite")
      .and_then(Value::as_str)
      .map(ToString::to_string),
  })
}

fn parse_set_cookie(url: &str, raw: &str) -> Option<Cookie> {
  let host = url::Url::parse(url).ok()?.host_str()?.to_string();
  let mut parts = raw.split(';').map(str::trim);
  let first = parts.next()?;
  let (name, value) = first.split_once('=')?;
  if name.is_empty() || value.is_empty() {
    return None;
  }
  let mut cookie = Cookie {
    name: name.to_string(),
    value: value.to_string(),
    domain: host,
    path: "/".to_string(),
    expires: Some(-1.0),
    secure: false,
    http_only: false,
    same_site: None,
  };
  for attr in parts {
    let (key, value) = attr.split_once('=').unwrap_or((attr, ""));
    match key.to_ascii_lowercase().as_str() {
      "domain" => cookie.domain = value.to_string(),
      "path" => {
        cookie.path = if value.is_empty() {
          "/".to_string()
        } else {
          value.to_string()
        }
      }
      "secure" => cookie.secure = true,
      "httponly" => cookie.http_only = true,
      "samesite" => cookie.same_site = Some(value.to_string()),
      "max-age" => {
        if let Ok(seconds) = value.parse::<i64>() {
          cookie.expires = Some(now() + seconds as f64);
        }
      }
      "expires" => {
        if let Ok(time) = httpdate::parse_http_date(value) {
          if let Ok(duration) = time.duration_since(UNIX_EPOCH) {
            cookie.expires = Some(duration.as_secs_f64());
          }
        }
      }
      _ => {}
    }
  }
  Some(cookie)
}

fn parse_set_cookie_headers(url: &str, headers: &HeaderMap) -> Vec<Cookie> {
  headers
    .get_all(SET_COOKIE)
    .iter()
    .filter_map(|value| value.to_str().ok())
    .filter_map(|raw| parse_set_cookie(url, raw))
    .collect()
}

pub struct HttpClient {
  pub auth: CookieAuth,
  cache_path: Option<PathBuf>,
  client: Client,
}

impl HttpClient {
  pub fn new(auth: CookieAuth, cache_path: Option<PathBuf>) -> Result<Self> {
    Ok(Self {
      auth,
      cache_path,
      client: Client::builder().no_proxy().build()?,
    })
  }

  pub fn request(
    &mut self,
    method: Method,
    url: &str,
    data: Option<Vec<u8>>,
    json_data: Option<Value>,
    headers: &[(&str, &str)],
    timeout: Duration,
    expect_json: bool,
  ) -> Result<Value> {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(
      reqwest::header::USER_AGENT,
      HeaderValue::from_static("Mozilla/5.0 gitcode-jupyter-tool"),
    );
    request_headers.insert(
      reqwest::header::ACCEPT,
      HeaderValue::from_static("application/json, text/plain, */*"),
    );
    let cookie_header = self.auth.cookie_header(url);
    if !cookie_header.is_empty() {
      request_headers.insert(
        reqwest::header::COOKIE,
        HeaderValue::from_str(&cookie_header)?,
      );
    }
    for (name, value) in headers {
      request_headers.insert(
        HeaderName::from_bytes(name.as_bytes())?,
        HeaderValue::from_str(value)?,
      );
    }
    let body = if let Some(json_data) = json_data {
      request_headers
        .entry(reqwest::header::CONTENT_TYPE)
        .or_insert(HeaderValue::from_static("application/json"));
      Some(serde_json::to_vec(&json_data)?)
    } else {
      data
    };
    let xsrf = self.auth.xsrf_token(url);
    if !xsrf.is_empty() && !matches!(method, Method::GET | Method::HEAD | Method::OPTIONS) {
      request_headers
        .entry(HeaderName::from_static("x-xsrftoken"))
        .or_insert(HeaderValue::from_str(&xsrf)?);
    }

    let mut request = self
      .client
      .request(method.clone(), url)
      .headers(request_headers)
      .timeout(timeout);
    if let Some(body) = body {
      request = request.body(body);
    }
    let response = request
      .send()
      .with_context(|| format!("{} {url}", method.as_str()))?;
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.bytes()?;
    self.update_cookies(url, &headers)?;
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
      bail!(
        "authenticated request rejected with HTTP {}: {url}",
        status.as_u16()
      );
    }
    if !status.is_success() {
      let body = String::from_utf8_lossy(&bytes);
      bail!(
        "HTTP {}: {url}: {}",
        status.as_u16(),
        body.chars().take(1000).collect::<String>()
      );
    }
    if !expect_json {
      return Ok(json!({"bytes_b64": STANDARD.encode(&bytes)}));
    }
    if bytes.is_empty() {
      return Ok(Value::Null);
    }
    Ok(serde_json::from_slice(&bytes).with_context(|| format!("expected JSON from {url}"))?)
  }

  fn update_cookies(&mut self, url: &str, headers: &HeaderMap) -> Result<()> {
    let cookies = parse_set_cookie_headers(url, headers);
    if cookies.is_empty() {
      return Ok(());
    }
    self.auth.update(cookies);
    if let Some(path) = &self.cache_path {
      self.auth.save(path)?;
    }
    Ok(())
  }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct NotebookInfo {
  pub notebook_id: String,
  pub learning_id: String,
  pub notebook_name: String,
  pub provisioning_status: String,
  pub base_url: String,
  pub lab_url: String,
  pub target_url: String,
  pub raw: Value,
}

pub fn extract_cookies_from_cdp(cdp_list_url: &str, debug: bool) -> Result<CookieAuth> {
  let pages: Vec<Value> = fetch_targets(cdp_list_url)?
    .into_iter()
    .filter(|target| target.get("type").and_then(Value::as_str) == Some("page"))
    .collect();
  let first = pages
    .first()
    .ok_or_else(|| anyhow!("no Chrome page is available for cookie extraction"))?;
  let websocket_url = first
    .get("webSocketDebuggerUrl")
    .and_then(Value::as_str)
    .ok_or_else(|| anyhow!("Chrome target has no webSocketDebuggerUrl"))?;
  let mut cdp = CdpClient::connect(websocket_url, debug)?;
  let result = (|| {
    cdp.call("Page.enable", json!({}), Duration::from_secs(10))?;
    cdp.call("Network.enable", json!({}), Duration::from_secs(10))?;
    cdp.call("Runtime.enable", json!({}), Duration::from_secs(10))?;
    cdp.pump_for(Duration::from_secs(1))?;
    let cookies = cdp.call("Network.getAllCookies", json!({}), Duration::from_secs(10))?;
    let gitcode_cookies: Vec<Cookie> = cookies
      .get("cookies")
      .and_then(Value::as_array)
      .into_iter()
      .flatten()
      .filter_map(normalize_cdp_cookie)
      .filter(|cookie| is_gitcode_domain(&cookie.domain))
      .collect();
    let auth = CookieAuth {
      cookies: gitcode_cookies,
      source: "chrome-cdp".to_string(),
    };
    if !auth.valid(0.0) {
      bail!("Chrome profile does not contain valid GitCode login cookies");
    }
    Ok(auth)
  })();
  cdp.close();
  result
}

pub fn notebook_from_insert(
  info: Value,
  notebook_path: &str,
  user_name: &str,
) -> Result<NotebookInfo> {
  let notebook_id = info
    .get("notebook_id")
    .and_then(Value::as_str)
    .ok_or_else(|| anyhow!("GitCode insert response has no notebook_id: {info}"))?
    .to_string();
  let re = regex::Regex::new(r"(?P<learning>.+)-(?P<name>notebook\d+)-(?P<suffix>[^-]+)$")?;
  let captures = re
    .captures(&notebook_id)
    .ok_or_else(|| anyhow!("unexpected GitCode notebook_id format: {notebook_id}"))?;
  let learning_id = captures["learning"].to_string();
  let notebook_name = captures["name"].to_string();
  let base_url = format!("https://aihub-run.gitcode.com/learning/{learning_id}/{notebook_name}");
  let lab_path = notebook_path.trim_matches('/');
  let mut lab_url = format!("{base_url}/lab");
  if !lab_path.is_empty() {
    lab_url.push_str(&format!("/tree/{lab_path}"));
  }
  let target_url = format!(
    "https://ai.gitcode.com/user/{}/notebookcann/lab?cannNotebookId={}",
    urlencoding::encode(user_name),
    urlencoding::encode(&notebook_id)
  );
  Ok(NotebookInfo {
    notebook_id,
    learning_id,
    notebook_name,
    provisioning_status: info
      .get("provisioning_status")
      .and_then(Value::as_str)
      .unwrap_or("")
      .to_string(),
    base_url,
    lab_url,
    target_url,
    raw: info,
  })
}

pub fn notebook_from_lab_url(href: &str, target_url: &str, raw: Value) -> Result<NotebookInfo> {
  let parsed = url::Url::parse(href)?;
  let re = regex::Regex::new(r"^/learning/([^/]+)/(notebook\d+)(?:/|$)")?;
  let captures = re
    .captures(parsed.path())
    .ok_or_else(|| anyhow!("state href is not a GitCode notebook URL: {href}"))?;
  let learning_id = captures[1].to_string();
  let notebook_name = captures[2].to_string();
  let base_url = format!(
    "{}://{}/learning/{}/{}",
    parsed.scheme(),
    parsed.host_str().unwrap_or("aihub-run.gitcode.com"),
    learning_id,
    notebook_name
  );
  let notebook_id = url::Url::parse(target_url)
    .ok()
    .and_then(|url| {
      url
        .query_pairs()
        .find(|(key, _)| key == "cannNotebookId")
        .map(|(_, value)| value.to_string())
    })
    .unwrap_or_else(|| format!("{learning_id}-{notebook_name}"));
  Ok(NotebookInfo {
    notebook_id,
    learning_id,
    notebook_name,
    provisioning_status: "READY".to_string(),
    base_url,
    lab_url: href.to_string(),
    target_url: target_url.to_string(),
    raw,
  })
}

pub fn insert_notebook(
  client: &mut HttpClient,
  repo_url: &str,
  ttl: &str,
  disk_size: &str,
  notebook_path: &str,
  scan_file_path: &str,
  gitcode_user: &str,
  timeout: Duration,
) -> Result<NotebookInfo> {
  let params = [
    ("repoUrl", repo_url),
    ("ttl", ttl),
    ("diskSize", disk_size),
    ("path", notebook_path),
    ("scanFilePath", scan_file_path),
    ("__s", "aihub"),
  ];
  let query = params
    .iter()
    .map(|(key, value)| {
      format!(
        "{}={}",
        urlencoding::encode(key),
        urlencoding::encode(value)
      )
    })
    .collect::<Vec<_>>()
    .join("&");
  let url = format!("https://web-api.gitcode.com/aihub/api/v1/notebookcann/insert?{query}");
  let info = client.request(
    Method::GET,
    &url,
    None,
    None,
    &[
      ("Origin", "https://ai.gitcode.com"),
      ("Referer", "https://ai.gitcode.com/"),
    ],
    timeout,
    true,
  )?;
  notebook_from_insert(info, notebook_path, gitcode_user)
}

pub fn probe_notebook(
  client: &mut HttpClient,
  notebook: &NotebookInfo,
  timeout: Duration,
) -> Result<Value> {
  client.request(
    Method::GET,
    &(notebook.base_url.clone() + "/api/status"),
    None,
    None,
    &[
      ("Origin", "https://aihub-run.gitcode.com"),
      ("Referer", &notebook.lab_url),
    ],
    timeout,
    true,
  )
}

pub struct ApiTerminal {
  pub client: HttpClient,
  pub notebook: NotebookInfo,
  pub rows: u16,
  pub cols: u16,
  pub name: String,
  pub closed: bool,
  ws: Option<WebSocket>,
}

impl ApiTerminal {
  pub fn new(client: HttpClient, notebook: NotebookInfo, rows: u16, cols: u16) -> Self {
    Self {
      client,
      notebook,
      rows,
      cols,
      name: String::new(),
      closed: false,
      ws: None,
    }
  }

  pub fn start(&mut self, timeout: Duration) -> Result<Value> {
    let model = self.client.request(
      Method::POST,
      &(self.notebook.base_url.clone() + "/api/terminals"),
      None,
      Some(json!({})),
      &[
        ("Origin", "https://aihub-run.gitcode.com"),
        ("Referer", &self.notebook.lab_url),
      ],
      timeout,
      true,
    )?;
    self.name = model
      .get("name")
      .and_then(Value::as_str)
      .ok_or_else(|| anyhow!("terminal create response has no name: {model}"))?
      .to_string();
    let websocket_url = format!(
      "wss://aihub-run.gitcode.com/learning/{}/{}/terminals/websocket/{}",
      self.notebook.learning_id,
      self.notebook.notebook_name,
      urlencoding::encode(&self.name)
    );
    let cookie_url = self.notebook.base_url.clone() + "/terminals/websocket/" + &self.name;
    let cookie_header = self.client.auth.cookie_header(&cookie_url);
    self.ws = Some(WebSocket::connect(
      &websocket_url,
      &[
        ("Cookie", cookie_header),
        ("Origin", "https://aihub-run.gitcode.com".to_string()),
      ],
      timeout,
    )?);
    self.send_json(&json!(["set_size", self.rows, self.cols]))?;
    Ok(json!({"name": self.name, "base": self.notebook.base_url, "href": self.notebook.lab_url}))
  }

  pub fn send_json(&mut self, payload: &Value) -> Result<()> {
    let ws = self
      .ws
      .as_mut()
      .ok_or_else(|| anyhow!("terminal websocket is not open"))?;
    ws.send_text(&serde_json::to_string(payload)?)?;
    Ok(())
  }

  pub fn send(&mut self, text: &str) -> Result<()> {
    self.send_json(&json!(["stdin", text]))
  }

  pub fn set_size(&mut self, rows: u16, cols: u16) -> Result<()> {
    self.rows = rows;
    self.cols = cols;
    self.send_json(&json!(["set_size", rows, cols]))
  }

  pub fn read_once(&mut self, timeout: Duration) -> Result<String> {
    if self.closed {
      return Ok(String::new());
    }
    let Some(ws) = self.ws.as_mut() else {
      return Ok(String::new());
    };
    let Some(message) = ws.recv_text(timeout)? else {
      return Ok(String::new());
    };
    let value: Value = match serde_json::from_str(&message) {
      Ok(value) => value,
      Err(_) => return Ok(message),
    };
    if let Some(text) = value
      .as_array()
      .and_then(|items| items.get(1))
      .and_then(Value::as_str)
    {
      return Ok(text.to_string());
    }
    Ok(String::new())
  }

  pub fn close(&mut self) {
    self.closed = true;
    if let Some(ws) = self.ws.as_mut() {
      ws.close();
    }
    self.ws = None;
    if !self.name.is_empty() {
      let url =
        self.notebook.base_url.clone() + "/api/terminals/" + &urlencoding::encode(&self.name);
      let _ = self.client.request(
        Method::DELETE,
        &url,
        None,
        None,
        &[
          ("Origin", "https://aihub-run.gitcode.com"),
          ("Referer", &self.notebook.lab_url),
        ],
        Duration::from_secs(10),
        false,
      );
      self.name.clear();
    }
  }
}
