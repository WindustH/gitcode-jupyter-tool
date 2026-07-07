mod args;
mod http;
mod jobs;
mod stream;
mod terminal;

use crate::config;
use crate::direct::{self, ApiTerminal, CookieAuth, HttpClient, NotebookInfo};
use crate::runtime;
use crate::util::{log, shell_quote, token_hex, write_json_file};
use anyhow::{Context, Result, anyhow, bail};
use args::Args;
use clap::Parser;
use http::run_http_server;
use jobs::JobManager;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};
use stream::run_stream_server;
use terminal::{LiveTerminal, drain_api_terminal, wait_api_command_marker};

const DOWNLOAD_BEGIN_MARK: &str = "__GJTD_DOWNLOAD_BEGIN__";
const DOWNLOAD_END_MARK: &str = "__GJTD_DOWNLOAD_END__";
struct DirectContext {
  client: HttpClient,
  notebook: NotebookInfo,
  status: Value,
  probe: Value,
}

struct DaemonContext {
  args: Args,
  chrome: Mutex<Option<Child>>,
}

impl DaemonContext {
  fn new(args: Args) -> Self {
    Self {
      args,
      chrome: Mutex::new(None),
    }
  }

  fn auth_cache_path(&self) -> PathBuf {
    config::expand_tilde(&self.args.auth_cache)
  }

  fn state_file_path(&self) -> PathBuf {
    config::expand_tilde(&self.args.state_file)
  }

  fn prepare_chrome_user_data_dir(&self) -> Result<PathBuf> {
    let profile = config::expand_tilde(&self.args.chrome_user_data_dir);
    fs::create_dir_all(&profile)?;
    for name in ["SingletonCookie", "SingletonLock", "SingletonSocket"] {
      let _ = fs::remove_file(profile.join(name));
    }
    Ok(profile)
  }

  fn chrome_environment(&self, headless: bool) -> Vec<(String, String)> {
    if headless
      || std::env::var_os("DISPLAY").is_some()
      || std::env::var_os("WAYLAND_DISPLAY").is_some()
    {
      return Vec::new();
    }
    let mut values = Vec::new();
    let uid = unsafe { libc::getuid() };
    let runtime = PathBuf::from(format!("/run/user/{uid}"));
    if runtime.exists() {
      values.push(("XDG_RUNTIME_DIR".to_string(), runtime.display().to_string()));
      if runtime.join("wayland-0").exists() {
        values.push(("WAYLAND_DISPLAY".to_string(), "wayland-0".to_string()));
        values.push(("XDG_SESSION_TYPE".to_string(), "wayland".to_string()));
      }
      if let Ok(mut entries) = fs::read_dir(&runtime).map(|iter| {
        iter
          .filter_map(Result::ok)
          .map(|entry| entry.path())
          .filter(|path| {
            path
              .file_name()
              .and_then(|s| s.to_str())
              .is_some_and(|name| name.starts_with("xauth_"))
          })
          .collect::<Vec<_>>()
      }) {
        entries.sort();
        if let Some(first) = entries.first() {
          values.push(("XAUTHORITY".to_string(), first.display().to_string()));
        }
      }
    }
    values.push(("DISPLAY".to_string(), ":1".to_string()));
    values.push((
      "DBUS_SESSION_BUS_ADDRESS".to_string(),
      format!("unix:path=/run/user/{uid}/bus"),
    ));
    values
  }

  fn stop_chrome(&self) {
    let mut guard = self.chrome.lock().unwrap();
    if let Some(child) = guard.as_mut() {
      let _ = child.kill();
      let _ = child.wait();
    }
    *guard = None;
  }

  fn launch_chrome(&self, headless: bool) -> Result<()> {
    let user_data_dir = self.prepare_chrome_user_data_dir()?;
    let mut command = Command::new(&self.args.chrome_bin);
    command
      .arg(format!("--remote-debugging-port={}", self.args.cdp_port))
      .arg("--remote-debugging-address=127.0.0.1")
      .arg(format!("--user-data-dir={}", user_data_dir.display()))
      .arg(format!(
        "--profile-directory={}",
        self.args.profile_directory
      ))
      .arg("--no-first-run");
    if headless {
      command
        .arg("--headless=new")
        .arg("--disable-gpu")
        .arg("--disable-dev-shm-usage")
        .arg(format!("--window-size={}", self.args.window_size));
      log("launching headless Chrome with gitcode-jupyter-tool profile");
    } else {
      command.arg("--new-window");
      log("launching visible Chrome with gitcode-jupyter-tool profile");
    }
    command.arg(&self.args.hub_url);
    for (key, value) in self.chrome_environment(headless) {
      command.env(key, value);
    }
    let child = command.spawn().context("launch Chrome")?;
    *self.chrome.lock().unwrap() = Some(child);
    Ok(())
  }

  fn ensure_cdp(&self, headless: bool) -> Result<()> {
    if runtime::fetch_targets(&self.args.cdp_list_url).is_ok() {
      return Ok(());
    }
    if self.args.no_launch {
      bail!("Chrome DevTools is not reachable");
    }
    let should_launch = {
      let mut guard = self.chrome.lock().unwrap();
      match guard.as_mut() {
        Some(child) => child.try_wait()?.is_some(),
        None => true,
      }
    };
    if should_launch {
      self.launch_chrome(headless)?;
    } else {
      log("waiting for existing Chrome process to expose DevTools");
    }
    let deadline = Instant::now() + Duration::from_secs_f64(self.args.chrome_start_timeout);
    let mut last_error: Option<String> = None;
    while Instant::now() < deadline {
      match runtime::fetch_targets(&self.args.cdp_list_url) {
        Ok(_) => return Ok(()),
        Err(err) => {
          last_error = Some(err.to_string());
          thread::sleep(Duration::from_millis(500));
        }
      }
    }
    self.stop_chrome();
    bail!(
      "Chrome DevTools did not become reachable. Last error: {}",
      last_error.unwrap_or_else(|| "unknown".to_string())
    )
  }

  fn load_cached_auth(&self) -> Option<CookieAuth> {
    let path = self.auth_cache_path();
    if !path.exists() {
      return None;
    }
    match CookieAuth::from_cache(&path) {
      Ok(auth) if auth.valid(self.args.auth_refresh_margin) => Some(auth),
      Ok(_) => None,
      Err(err) => {
        log(format!("ignored unreadable auth cache: {err}"));
        None
      }
    }
  }

  fn extract_auth_from_chrome(&self, headless: bool) -> Result<CookieAuth> {
    self.ensure_cdp(headless)?;
    let mut auth = match direct::extract_cookies_from_cdp(&self.args.cdp_list_url, self.args.debug)
    {
      Ok(auth) => auth,
      Err(_) => {
        let _ = runtime::open_new_tab(&self.args.cdp_list_url, &self.args.hub_url);
        thread::sleep(Duration::from_secs_f64(self.args.hub_load_delay));
        direct::extract_cookies_from_cdp(&self.args.cdp_list_url, self.args.debug)?
      }
    };
    auth.source = "chrome-cdp".to_string();
    auth.save(&self.auth_cache_path())?;
    log(format!(
      "cached GitCode auth from Chrome profile: {} cookie names",
      auth.redacted_names().len()
    ));
    Ok(auth)
  }

  fn get_direct_auth(
    &self,
    force_extract: bool,
    allow_chrome: bool,
    headless: bool,
  ) -> Result<CookieAuth> {
    if !force_extract {
      if let Some(auth) = self.load_cached_auth() {
        return Ok(auth);
      }
    }
    if !allow_chrome {
      bail!("GitCode auth cache is missing or expired");
    }
    self.extract_auth_from_chrome(headless)
  }

  fn previous_notebook_from_state(&self) -> Option<NotebookInfo> {
    let path = self.state_file_path();
    let text = fs::read_to_string(&path).ok()?;
    let data: Value = serde_json::from_str(&text).ok()?;
    let href = data.get("href")?.as_str()?.to_string();
    if !href.contains("aihub-run.gitcode.com") {
      return None;
    }
    direct::notebook_from_lab_url(
      &href,
      data.get("target_url").and_then(Value::as_str).unwrap_or(""),
      json!({
          "state_file": path.display().to_string(),
          "state_time": data.get("time").cloned().unwrap_or(Value::Null),
          "status": data.get("status").cloned().unwrap_or_else(|| json!({})),
          "notebook_id": data.get("notebook_id").cloned().unwrap_or(Value::Null),
      }),
    )
    .ok()
  }

  fn direct_probe_output(&self, status: &Value, notebook: &NotebookInfo) -> String {
    let mut pieces = vec![format!("notebook_id={}", notebook.notebook_id)];
    for key in ["connections", "kernels", "started", "last_activity"] {
      if let Some(value) = status.get(key) {
        pieces.push(format!(
          "{key}={}",
          value
            .as_str()
            .map_or_else(|| value.to_string(), ToString::to_string)
        ));
      }
    }
    pieces.join("\n")
  }

  fn direct_probe_context(
    &self,
    mut client: HttpClient,
    notebook: NotebookInfo,
  ) -> Result<DirectContext> {
    let status = direct::probe_notebook(
      &mut client,
      &notebook,
      Duration::from_secs_f64(self.args.probe_timeout),
    )?;
    let previous_started = notebook
      .raw
      .pointer("/status/started")
      .and_then(Value::as_str)
      .unwrap_or("");
    let current_started = status.get("started").and_then(Value::as_str).unwrap_or("");
    if !previous_started.is_empty()
      && !current_started.is_empty()
      && previous_started != current_started
    {
      log(format!(
        "notebook backend restarted; remote temporary files may have been reset: {} previous_started={} current_started={}",
        notebook.base_url, previous_started, current_started
      ));
    }
    let output = self.direct_probe_output(&status, &notebook);
    let probe = json!({
        "ok": true,
        "href": notebook.lab_url,
        "target_url": notebook.target_url,
        "notebook_id": notebook.notebook_id,
        "base_url": notebook.base_url,
        "status": status,
        "output": output,
    });
    Ok(DirectContext {
      client,
      notebook,
      status,
      probe,
    })
  }

  fn direct_context(
    &self,
    timeout: Duration,
    force_auth_extract: bool,
    allow_chrome: bool,
    headless: bool,
    skip_previous: bool,
  ) -> Result<DirectContext> {
    let deadline = Instant::now() + timeout;
    let mut last_error: String;
    let mut force_extract_next = force_auth_extract;
    let mut previous_dead = skip_previous;
    loop {
      let attempt = (|| {
        let auth = self.get_direct_auth(force_extract_next, allow_chrome, headless)?;
        let client = HttpClient::new(auth, Some(self.auth_cache_path()))?;
        if !previous_dead {
          if let Some(previous) = self.previous_notebook_from_state() {
            match self.direct_probe_context(client, previous.clone()) {
              Ok(context) => {
                log(format!("reusing previous notebook: {}", previous.base_url));
                return Ok(context);
              }
              Err(err) => {
                previous_dead = true;
                log(format!("previous notebook is not reusable: {err}"));
              }
            }
          }
        }
        let auth = self.get_direct_auth(force_extract_next, allow_chrome, headless)?;
        let mut client = HttpClient::new(auth, Some(self.auth_cache_path()))?;
        let notebook = direct::insert_notebook(
          &mut client,
          &self.args.repo_url,
          &self.args.ttl,
          &self.args.disk_size,
          &self.args.notebook_path,
          &self.args.scan_file_path,
          &self.args.gitcode_user,
          Duration::from_secs_f64(self.args.insert_timeout),
        )?;
        if !notebook.provisioning_status.is_empty() && notebook.provisioning_status != "READY" {
          bail!(
            "notebook is not ready: status={} queue={}",
            notebook.provisioning_status,
            notebook
              .raw
              .get("queue_ahead_count")
              .cloned()
              .unwrap_or(Value::Null)
          );
        }
        self.direct_probe_context(client, notebook)
      })();

      match attempt {
        Ok(context) => return Ok(context),
        Err(err) => {
          if allow_chrome && !force_extract_next && err.to_string().contains("auth") {
            force_extract_next = true;
          }
          last_error = err.to_string();
        }
      }
      if Instant::now() >= deadline {
        bail!("direct notebook probe failed: {}", last_error.clone());
      }
      thread::sleep(Duration::from_secs_f64(self.args.create_probe_interval));
    }
  }

  fn write_state(&self, data: Value) -> Result<()> {
    if self.args.state_file.is_empty() {
      return Ok(());
    }
    write_json_file(&self.state_file_path(), &data)
  }

  fn record_ok_state(&self, probe: &Value) -> Result<()> {
    let status = probe.get("status").cloned().unwrap_or_else(|| json!({}));
    self.write_state(json!({
        "ok": true,
        "href": probe.get("href").cloned().unwrap_or(Value::Null),
        "target_url": probe.get("target_url").cloned().unwrap_or(Value::Null),
        "notebook_id": probe.get("notebook_id").cloned().unwrap_or(Value::Null),
        "base_url": probe.get("base_url").cloned().unwrap_or(Value::Null),
        "status": status,
        "remote_started": status.get("started").cloned().unwrap_or(Value::Null),
        "remote_last_activity": status.get("last_activity").cloned().unwrap_or(Value::Null),
        "target_url_contains": self.args.notebook_target_contains,
        "page_url_contains": self.args.notebook_page_contains,
        "profile": config::expand_tilde(&self.args.chrome_user_data_dir).display().to_string(),
        "probe": probe.get("output").cloned().unwrap_or(Value::Null),
        "time": direct::now(),
    }))
  }

  fn open_login_window_and_wait(&self, reason: &str) -> Result<bool> {
    if self.args.no_login_window {
      bail!("notebook is unavailable and login window is disabled: {reason}");
    }
    log(format!(
      "opening visible login window because notebook is unavailable: {reason}"
    ));
    self.stop_chrome();
    self.ensure_cdp(false)?;
    log(
      "GitCode login may be required. Complete login in the visible Chrome window; gjtd will keep polling.",
    );
    let deadline = Instant::now() + Duration::from_secs_f64(self.args.login_timeout);
    let mut last_error = String::new();
    while Instant::now() < deadline {
      match self.direct_context(
        Duration::from_secs_f64(self.args.login_probe_interval.max(1.0)),
        true,
        true,
        false,
        false,
      ) {
        Ok(context) => {
          log(format!(
            "notebook ok after login: {} | {}",
            context
              .probe
              .get("href")
              .and_then(Value::as_str)
              .unwrap_or(""),
            context
              .probe
              .get("output")
              .and_then(Value::as_str)
              .unwrap_or("")
              .replace('\n', " ; ")
          ));
          self.record_ok_state(&context.probe)?;
          return Ok(true);
        }
        Err(err) => last_error = err.to_string(),
      }
      thread::sleep(Duration::from_secs_f64(self.args.login_probe_interval));
    }
    bail!("visible login window was opened, but notebook is still unavailable: {last_error}")
  }

  fn maintain_once(&self) -> Result<bool> {
    match self.direct_context(
      Duration::from_secs_f64(self.args.direct_timeout),
      false,
      !self.args.no_launch,
      self.args.headless(),
      false,
    ) {
      Ok(context) => {
        log(format!(
          "notebook ok: {} | {}",
          context
            .probe
            .get("href")
            .and_then(Value::as_str)
            .unwrap_or(""),
          context
            .probe
            .get("output")
            .and_then(Value::as_str)
            .unwrap_or("")
            .replace('\n', " ; ")
        ));
        self.record_ok_state(&context.probe)?;
        return Ok(true);
      }
      Err(err) => {
        log(format!("notebook not ready: {err}"));
        if self.args.status_only {
          self.write_state(json!({"ok": false, "reason": "status_only", "time": direct::now()}))?;
          return Ok(false);
        }
        self.write_state(
          json!({"ok": false, "reason": "direct_probe_failed", "time": direct::now()}),
        )?;
        if !self.args.no_login_window && !self.args.no_launch {
          return self.open_login_window_and_wait("direct probe failed");
        }
        bail!("direct notebook probe failed and browser launch is disabled");
      }
    }
  }
}

struct Service {
  context: Arc<DaemonContext>,
  jobs: JobManager,
  ensure_lock: Mutex<()>,
  terminals: RwLock<HashMap<String, Arc<LiveTerminal>>>,
}

impl Service {
  fn new(context: Arc<DaemonContext>) -> Self {
    let retention = Duration::from_secs_f64(context.args.job_retention);
    Self {
      context,
      jobs: JobManager::new(retention),
      ensure_lock: Mutex::new(()),
      terminals: RwLock::new(HashMap::new()),
    }
  }

  fn direct_context(&self, skip_previous: bool) -> Result<DirectContext> {
    let _guard = self.ensure_lock.lock().unwrap();
    let context = self.context.direct_context(
      Duration::from_secs_f64(self.context.args.create_timeout),
      false,
      !self.context.args.no_launch,
      self.context.args.headless(),
      skip_previous,
    )?;
    self.context.record_ok_state(&context.probe)?;
    Ok(context)
  }

  fn ensure(&self) -> Result<Value> {
    let context = self.direct_context(false)?;
    Ok(json!({"ok": true, "href": context.notebook.lab_url, "status": context.status}))
  }

  fn run_command(
    &self,
    command: String,
    timeout: Duration,
    rows: u16,
    cols: u16,
    raw: bool,
    no_prelude: bool,
    ensure: bool,
  ) -> Result<Value> {
    let mut last_error: Option<anyhow::Error> = None;
    for attempt in 0..2 {
      let context = if ensure {
        self.direct_context(attempt > 0)?
      } else {
        self.context.direct_context(
          Duration::from_secs_f64(self.context.args.direct_timeout),
          false,
          false,
          self.context.args.headless(),
          attempt > 0,
        )?
      };
      let mut terminal = ApiTerminal::new(context.client, context.notebook.clone(), rows, cols);
      let marker = format!("__GJTD_RUN_{}__:", token_hex(8));
      match (|| {
        terminal.start(Duration::from_secs(30))?;
        drain_api_terminal(&mut terminal, Duration::from_millis(500))?;
        if !no_prelude {
          terminal.send("stty -echo 2>/dev/null || true\nexport PS1='' PS2=''\n")?;
          drain_api_terminal(&mut terminal, Duration::from_millis(200))?;
        }
        let wrapped = [
          "(".to_string(),
          command.clone(),
          ")".to_string(),
          "__gjtd_run_status=$?".to_string(),
          format!("printf '{}%s\\n' \"$__gjtd_run_status\"", marker),
          String::new(),
        ]
        .join("\n");
        terminal.send(&wrapped)?;
        let (status, text) = wait_api_command_marker(&mut terminal, &marker, timeout, raw)?;
        Ok(
          json!({"ok": status == 0, "exit_code": status, "stdout": text, "href": context.notebook.lab_url}),
        )
      })() {
        Ok(result) => {
          terminal.close();
          return Ok(result);
        }
        Err(err) => {
          terminal.close();
          last_error = Some(err);
          if attempt == 0 {
            log("remote command failed on current notebook; replacing notebook");
          }
        }
      }
    }
    bail!(
      "remote command failed after notebook replacement: {}",
      last_error
        .map(|err| err.to_string())
        .unwrap_or_else(|| "unknown".to_string())
    )
  }

  fn upload(
    &self,
    destination: String,
    payload_b64: String,
    mode: u32,
    is_archive: bool,
    timeout: Duration,
    chunk_size: usize,
  ) -> Result<Value> {
    let marker = format!("__GJTD_UPLOAD_{}__:", token_hex(8));
    let heredoc = format!("__GJTD_UPLOAD_PAYLOAD_{}__", token_hex(8));
    let destination_arg = shell_quote(&destination);
    let mode_octal = format!("{mode:o}");
    let remote_decode = if is_archive {
      format!(
        "mkdir -p {dest}\nbase64 -d \"$tmp_b64\" | tar -xzf - -C {dest}",
        dest = destination_arg
      )
    } else {
      format!(
        "mkdir -p \"$(dirname {dest})\"\nbase64 -d \"$tmp_b64\" > {dest}\nchmod {mode} {dest}",
        dest = destination_arg,
        mode = mode_octal
      )
    };
    let mut last_error: Option<anyhow::Error> = None;
    for attempt in 0..2 {
      let context = self.direct_context(attempt > 0)?;
      let mut terminal = ApiTerminal::new(context.client, context.notebook.clone(), 24, 120);
      let result = (|| {
        terminal.start(Duration::from_secs(30))?;
        drain_api_terminal(&mut terminal, Duration::from_millis(500))?;
        terminal.send("stty -echo 2>/dev/null || true\nexport PS1='' PS2=''\n")?;
        drain_api_terminal(&mut terminal, Duration::from_millis(200))?;
        terminal.send(
          &[
            "set +e".to_string(),
            "tmp_b64=$(mktemp /tmp/gjtd-upload.XXXXXX.b64)".to_string(),
            "cleanup_gjtd_upload() { rm -f \"$tmp_b64\"; }".to_string(),
            "trap cleanup_gjtd_upload EXIT".to_string(),
            format!("cat > \"$tmp_b64\" <<'{heredoc}'"),
            String::new(),
          ]
          .join("\n"),
        )?;
        for chunk in payload_b64.as_bytes().chunks(chunk_size.max(1)) {
          terminal.send(std::str::from_utf8(chunk)?)?;
        }
        if !payload_b64.ends_with('\n') {
          terminal.send("\n")?;
        }
        terminal.send(
          &[
            heredoc.clone(),
            remote_decode.clone(),
            "__gjtd_upload_status=$?".to_string(),
            format!("printf '{}%s\\n' \"$__gjtd_upload_status\"", marker),
            String::new(),
          ]
          .join("\n"),
        )?;
        let (status, text) = wait_api_command_marker(&mut terminal, &marker, timeout, false)?;
        Ok(
          json!({"ok": status == 0, "exit_code": status, "stdout": text, "href": context.notebook.lab_url}),
        )
      })();
      terminal.close();
      match result {
        Ok(value) => return Ok(value),
        Err(err) => {
          last_error = Some(err);
          if attempt == 0 {
            log("upload failed on current notebook; replacing notebook");
          }
        }
      }
    }
    bail!(
      "upload failed after notebook replacement: {}",
      last_error.map(|e| e.to_string()).unwrap_or_default()
    )
  }

  fn download(&self, source: String, recursive: bool, timeout: Duration) -> Result<Value> {
    let source_arg = shell_quote(&source);
    let command = if recursive {
      format!(
        r#"source_path={source}
if [ ! -d "$source_path" ]; then
  printf '%s\n' "$source_path is not a directory; use without -r to copy files" >&2
  exit 1
fi
printf '%s\n' {begin}
tar -czf - -C "$source_path" . | base64
printf '%s\n' {end}
"#,
        source = source_arg,
        begin = shell_quote(DOWNLOAD_BEGIN_MARK),
        end = shell_quote(DOWNLOAD_END_MARK),
      )
    } else {
      format!(
        r#"source_path={source}
if [ -d "$source_path" ]; then
  printf '%s\n' "$source_path is a directory; use -r to copy directories" >&2
  exit 1
fi
printf '%s\n' {begin}
base64 < "$source_path"
printf '%s\n' {end}
"#,
        source = source_arg,
        begin = shell_quote(DOWNLOAD_BEGIN_MARK),
        end = shell_quote(DOWNLOAD_END_MARK),
      )
    };
    let result = self.run_command(command, timeout, 24, 120, false, false, true)?;
    if !result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
      return Ok(result);
    }
    let stdout = result.get("stdout").and_then(Value::as_str).unwrap_or("");
    let Some(begin) = stdout.find(DOWNLOAD_BEGIN_MARK) else {
      return Ok(
        json!({"ok": false, "exit_code": 1, "stdout": stdout, "error": "remote download did not return a payload", "href": result.get("href").cloned().unwrap_or(Value::Null)}),
      );
    };
    let Some(end) = stdout.find(DOWNLOAD_END_MARK) else {
      return Ok(
        json!({"ok": false, "exit_code": 1, "stdout": stdout, "error": "remote download did not return a payload", "href": result.get("href").cloned().unwrap_or(Value::Null)}),
      );
    };
    if end < begin {
      return Ok(
        json!({"ok": false, "exit_code": 1, "stdout": stdout, "error": "remote download did not return a payload", "href": result.get("href").cloned().unwrap_or(Value::Null)}),
      );
    }
    let payload = stdout[begin + DOWNLOAD_BEGIN_MARK.len()..end]
      .split_whitespace()
      .collect::<String>();
    Ok(
      json!({"ok": true, "exit_code": 0, "content_b64": payload, "href": result.get("href").cloned().unwrap_or(Value::Null)}),
    )
  }

  fn open_terminal(&self, rows: u16, cols: u16) -> Result<Arc<LiveTerminal>> {
    match LiveTerminal::new(self, rows, cols) {
      Ok(terminal) => Ok(Arc::new(terminal)),
      Err(_) => {
        let _ = self.ensure();
        Ok(Arc::new(LiveTerminal::new(self, rows, cols)?))
      }
    }
  }

  fn start_terminal(&self, rows: u16, cols: u16) -> Result<Value> {
    let terminal = self.open_terminal(rows, cols)?;
    let initial_output = terminal.prepare_interactive_prompt()?;
    let id = terminal.id.clone();
    let href = terminal.href.clone();
    self.terminals.write().unwrap().insert(id.clone(), terminal);
    Ok(json!({"ok": true, "id": id, "href": href, "initial_output": initial_output}))
  }

  fn get_terminal(&self, id: &str) -> Result<Arc<LiveTerminal>> {
    self
      .terminals
      .read()
      .unwrap()
      .get(id)
      .cloned()
      .ok_or_else(|| anyhow!("unknown terminal id: {id}"))
  }

  fn close_terminal(&self, id: &str) {
    if let Some(terminal) = self.terminals.write().unwrap().remove(id) {
      terminal.close();
    }
  }

  fn close(&self) {
    let ids = self
      .terminals
      .read()
      .unwrap()
      .keys()
      .cloned()
      .collect::<Vec<_>>();
    for id in ids {
      self.close_terminal(&id);
    }
  }
}

fn run_server(context: Arc<DaemonContext>) -> Result<i32> {
  let service = Arc::new(Service::new(Arc::clone(&context)));
  let stop = Arc::new(AtomicBool::new(false));
  let stream_thread = {
    let service = Arc::clone(&service);
    let stop = Arc::clone(&stop);
    thread::spawn(move || run_stream_server(service, stop))
  };
  if context.args.interval > 0.0 {
    let service = Arc::clone(&service);
    let stop = Arc::clone(&stop);
    let interval = Duration::from_secs_f64(context.args.interval);
    thread::spawn(move || {
      while !stop.load(Ordering::Relaxed) {
        if let Err(err) = service.ensure() {
          log(format!("background maintenance failed: {err}"));
        }
        let deadline = Instant::now() + interval;
        while !stop.load(Ordering::Relaxed) && Instant::now() < deadline {
          thread::sleep(Duration::from_millis(200));
        }
      }
    });
  }
  let result = run_http_server(Arc::clone(&service), Arc::clone(&stop));
  stop.store(true, Ordering::Relaxed);
  let _ = stream_thread.join();
  service.close();
  context.stop_chrome();
  result?;
  Ok(0)
}

pub fn run_cli() {
  let args = Args::parse();
  let context = Arc::new(DaemonContext::new(args.clone()));
  let code = if !args.once && !args.status_only {
    run_server(context).unwrap_or_else(|err| {
      eprintln!("gjtd: {err:#}");
      1
    })
  } else {
    match context.maintain_once() {
      Ok(_) => 0,
      Err(err) => {
        log(format!("maintenance failed: {err}"));
        1
      }
    }
  };
  std::process::exit(code);
}
