use crate::config::{DEFAULT_API_URL, DEFAULT_LOG, DEFAULT_STREAM_URL, expand_tilde};
use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use serde_json::{Value, json};
use std::fs::OpenOptions;
use std::net::{TcpStream, ToSocketAddrs};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub fn request(api_url: &str, path: &str, payload: Value, timeout: Duration) -> Result<Value> {
  let url = format!("{}{}", api_url.trim_end_matches('/'), path);
  let client = Client::builder().no_proxy().timeout(timeout).build()?;
  let response = client
    .post(&url)
    .header(reqwest::header::CONTENT_TYPE, "application/json")
    .header(reqwest::header::ACCEPT, "application/json")
    .body(serde_json::to_vec(&payload)?)
    .send()
    .with_context(|| format!("gjtd request failed for {url}"))?;
  Ok(
    response
      .json()
      .with_context(|| format!("parse JSON response from {url}"))?,
  )
}

pub fn health(api_url: &str) -> bool {
  let url = format!("{}/v1/health", api_url.trim_end_matches('/'));
  let Ok(client) = Client::builder()
    .no_proxy()
    .timeout(Duration::from_secs(2))
    .build()
  else {
    return false;
  };
  let Ok(response) = client.get(url).send() else {
    return false;
  };
  let Ok(payload) = response.json::<Value>() else {
    return false;
  };
  payload.get("ok").and_then(Value::as_bool).unwrap_or(false)
}

pub fn wait_job_result(
  api_url: &str,
  job_id: &str,
  timeout: Duration,
  poll_interval: Duration,
) -> Result<Value> {
  let deadline = Instant::now() + timeout;
  while Instant::now() < deadline {
    let payload = request(
      api_url,
      "/v1/job",
      json!({"id": job_id}),
      Duration::from_secs(5),
    )?;
    if !payload.get("ok").and_then(Value::as_bool).unwrap_or(false) {
      bail!(
        "{}",
        payload
          .get("error")
          .and_then(Value::as_str)
          .unwrap_or("gjtd job query failed")
      );
    }
    let job = payload.get("job").cloned().unwrap_or_else(|| json!({}));
    match job.get("status").and_then(Value::as_str) {
      Some("succeeded") => {
        return Ok(
          job
            .get("result")
            .cloned()
            .unwrap_or_else(|| json!({"ok": true})),
        );
      }
      Some("failed") => {
        return Ok(json!({
            "ok": false,
            "error": job.get("error").and_then(Value::as_str).unwrap_or("gjtd job failed"),
        }));
      }
      _ => thread::sleep(poll_interval),
    }
  }
  Ok(
    json!({"ok": false, "error": format!("gjtd job {job_id} timed out after {:.1}s", timeout.as_secs_f64()), "exit_code": 124}),
  )
}

pub fn parse_tcp_url(url: &str) -> Result<(String, u16)> {
  if !url.contains("://") {
    let mut parts = url.rsplitn(2, ':');
    let port = parts
      .next()
      .ok_or_else(|| anyhow!("stream URL has no port: {url}"))?
      .parse::<u16>()?;
    let host = parts.next().unwrap_or("127.0.0.1").to_string();
    return Ok((host, port));
  }
  let parsed = url::Url::parse(url)?;
  if parsed.scheme() != "tcp" {
    bail!("unsupported stream URL scheme: {url}");
  }
  Ok((
    parsed.host_str().unwrap_or("127.0.0.1").to_string(),
    parsed.port().unwrap_or(18788),
  ))
}

pub fn connect_tcp(url: &str, timeout: Duration) -> Result<TcpStream> {
  let (host, port) = parse_tcp_url(url)?;
  let mut addrs = (host.as_str(), port).to_socket_addrs()?;
  let addr = addrs
    .next()
    .ok_or_else(|| anyhow!("no socket address for {host}:{port}"))?;
  Ok(TcpStream::connect_timeout(&addr, timeout)?)
}

pub fn default_api_url() -> String {
  crate::config::env_string(&["GJTD_API_URL", "JUPYTERD_API_URL"], DEFAULT_API_URL)
}

pub fn default_stream_url() -> String {
  crate::config::env_string(
    &["GJTD_STREAM_URL", "JUPYTERD_STREAM_URL"],
    DEFAULT_STREAM_URL,
  )
}

pub fn default_log() -> String {
  crate::config::env_string(&["GJTD_LOG", "JUPYTERD_LOG"], DEFAULT_LOG)
}

pub fn daemon_path() -> Result<PathBuf> {
  let current = std::env::current_exe()?;
  Ok(current.with_file_name("gjtd"))
}

pub fn start_daemon(
  api_url: &str,
  stream_url: &str,
  headless: bool,
  log_path: &str,
  timeout: Duration,
) -> Result<()> {
  if health(api_url) {
    return Ok(());
  }
  let parsed = url::Url::parse(api_url)?;
  let host = parsed.host_str().unwrap_or("127.0.0.1").to_string();
  let port = parsed.port().unwrap_or(18787).to_string();
  let (stream_host, stream_port) = parse_tcp_url(stream_url)?;
  let daemon = daemon_path()?;
  let log_path = expand_tilde(log_path);
  if let Some(parent) = log_path.parent() {
    std::fs::create_dir_all(parent)?;
  }
  let log = OpenOptions::new()
    .create(true)
    .append(true)
    .open(&log_path)?;
  let log_err = log.try_clone()?;
  let mut command = Command::new(daemon);
  command
    .arg("--listen-host")
    .arg(host)
    .arg("--listen-port")
    .arg(port)
    .arg("--stream-host")
    .arg(stream_host)
    .arg("--stream-port")
    .arg(stream_port.to_string())
    .stdout(Stdio::from(log))
    .stderr(Stdio::from(log_err));
  if headless {
    command.arg("--headless");
  } else {
    command.arg("--visible");
  }
  unsafe {
    command.pre_exec(|| {
      libc::setsid();
      Ok(())
    });
  }
  command.spawn().with_context(|| "start gjtd")?;

  let deadline = Instant::now() + timeout;
  while Instant::now() < deadline {
    if health(api_url) {
      return Ok(());
    }
    thread::sleep(Duration::from_millis(250));
  }
  bail!(
    "gjtd did not start within {:.1}s; log: {}",
    timeout.as_secs_f64(),
    log_path.display()
  )
}

pub fn ensure_daemon(
  api_url: &str,
  auto_start: bool,
  stream_url: &str,
  headless: bool,
  log_path: &str,
  timeout: Duration,
) -> Result<()> {
  if health(api_url) {
    return Ok(());
  }
  if auto_start {
    start_daemon(api_url, stream_url, headless, log_path, timeout)
  } else {
    bail!("gjtd is not running at {api_url}; start it with gjtctl start")
  }
}
