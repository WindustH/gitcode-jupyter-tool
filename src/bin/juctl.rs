use anyhow::Result;
use clap::{Args as ClapArgs, Parser, Subcommand};
use gitcode_jupyter_tool::{client, config, direct::CookieAuth};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "juctl", about = "Control the local jud daemon.")]
struct Cli {
  #[arg(long, global = true, default_value_t = config::default_account())]
  account: String,
  #[command(subcommand)]
  command: CommandKind,
}

#[derive(Subcommand)]
enum CommandKind {
  Accounts {
    #[command(subcommand)]
    command: AccountsCommand,
  },
  Status(StatusArgs),
  Start(CommonArgs),
  Stop(StopArgs),
  Restart(StopArgs),
  Login(LoginArgs),
  Logout(LogoutArgs),
  Resources(CommonArgs),
  Reset(CommonArgs),
}

#[derive(Subcommand)]
enum AccountsCommand {
  /// List configured accounts and their cached login state.
  List,
}

#[derive(Clone, ClapArgs)]
struct CommonArgs {
  #[arg(long, default_value_t = client::default_api_url())]
  daemon_url: String,
  #[arg(long, default_value_t = client::default_stream_url())]
  stream_url: String,
  #[arg(long, default_value_t = client::default_log())]
  daemon_log: String,
  #[arg(long, default_value_t = 10.0)]
  timeout: f64,
  #[arg(long, default_value_t = true)]
  headless: bool,
  #[arg(long, action = clap::ArgAction::SetTrue)]
  visible: bool,
}

#[derive(Clone, ClapArgs)]
struct StatusArgs {
  #[command(flatten)]
  common: CommonArgs,
  #[arg(long, action = clap::ArgAction::SetTrue)]
  json: bool,
}

#[derive(Clone, ClapArgs)]
struct StopArgs {
  #[command(flatten)]
  common: CommonArgs,
  #[arg(long, action = clap::ArgAction::SetTrue)]
  force: bool,
}

#[derive(Clone, ClapArgs)]
struct LoginArgs {
  #[arg(long, default_value_t = client::default_api_url())]
  daemon_url: String,
  #[arg(long, default_value_t = client::default_stream_url())]
  stream_url: String,
  #[arg(long, default_value_t = client::default_log())]
  daemon_log: String,
  #[arg(long, default_value_t = 300.0)]
  timeout: f64,
  #[arg(long, action = clap::ArgAction::SetTrue)]
  no_restart: bool,
}

#[derive(Clone, ClapArgs)]
struct LogoutArgs {
  #[command(flatten)]
  common: CommonArgs,
  #[arg(long, action = clap::ArgAction::SetTrue)]
  force: bool,
  #[arg(long, action = clap::ArgAction::SetTrue)]
  keep_profile: bool,
}

impl CommonArgs {
  fn headless(&self) -> bool {
    if self.visible { false } else { self.headless }
  }
}

fn daemon_path() -> PathBuf {
  client::daemon_path().unwrap_or_else(|_| PathBuf::from("jud"))
}

fn proc_cmdline(proc_dir: &Path) -> Vec<String> {
  let Ok(raw) = fs::read(proc_dir.join("cmdline")) else {
    return Vec::new();
  };
  raw
    .split(|b| *b == 0)
    .filter(|part| !part.is_empty())
    .map(|part| String::from_utf8_lossy(part).into_owned())
    .collect()
}

fn proc_cwd(proc_dir: &Path) -> Option<PathBuf> {
  fs::read_link(proc_dir.join("cwd"))
    .ok()
    .and_then(|path| path.canonicalize().ok())
}

fn proc_exe(proc_dir: &Path) -> Option<PathBuf> {
  fs::read_link(proc_dir.join("exe"))
    .ok()
    .and_then(|path| path.canonicalize().ok())
}

fn is_this_jud(proc_dir: &Path, args: &[String], account: &str) -> bool {
  let daemon = daemon_path();
  let daemon_real = daemon.canonicalize().unwrap_or(daemon);
  let account_matches = args
    .windows(2)
    .any(|pair| pair[0] == "--account" && pair[1] == account)
    || (account == config::DEFAULT_ACCOUNT && !args.iter().any(|arg| arg == "--account"));
  if !account_matches {
    return false;
  }
  if proc_exe(proc_dir).is_some_and(|exe| exe == daemon_real) {
    return true;
  }
  let cwd = proc_cwd(proc_dir);
  for arg in args {
    let path = PathBuf::from(arg);
    if path.is_absolute() {
      if path
        .canonicalize()
        .is_ok_and(|candidate| candidate == daemon_real)
      {
        return true;
      }
    } else if let Some(cwd) = &cwd
      && cwd
        .join(&path)
        .canonicalize()
        .is_ok_and(|candidate| candidate == daemon_real)
    {
      return true;
    }
  }
  false
}

fn daemon_processes(account: &str) -> Vec<(i32, Vec<String>)> {
  let current = std::process::id() as i32;
  let mut matches = Vec::new();
  let Ok(entries) = fs::read_dir("/proc") else {
    return matches;
  };
  for entry in entries.flatten() {
    let name = entry.file_name();
    let Some(name) = name.to_str() else {
      continue;
    };
    let Ok(pid) = name.parse::<i32>() else {
      continue;
    };
    if pid == current {
      continue;
    }
    let proc_dir = entry.path();
    let args = proc_cmdline(&proc_dir);
    if !args.is_empty() && is_this_jud(&proc_dir, &args, account) {
      matches.push((pid, args));
    }
  }
  matches.sort_by_key(|(pid, _)| *pid);
  matches
}

fn proc_state(pid: i32) -> String {
  let Ok(text) = fs::read_to_string(format!("/proc/{pid}/status")) else {
    return String::new();
  };
  for line in text.lines() {
    if let Some(rest) = line.strip_prefix("State:") {
      return rest.split_whitespace().next().unwrap_or("?").to_string();
    }
  }
  "?".to_string()
}

fn wait_until_stopped(pids: &[i32], timeout: Duration) -> Vec<i32> {
  let deadline = Instant::now() + timeout;
  let mut remaining = pids.to_vec();
  while !remaining.is_empty() && Instant::now() < deadline {
    remaining.retain(|pid| unsafe { libc::kill(*pid, 0) } == 0);
    if !remaining.is_empty() {
      thread::sleep(Duration::from_millis(100));
    }
  }
  remaining
}

fn stop_via_api(api_url: &str, account: &str, timeout: Duration) -> bool {
  if let Some(payload) = client::health_payload(api_url) {
    let actual = payload.get("account").and_then(Value::as_str);
    let matches = actual
      .map(|value| value == account)
      .unwrap_or(account == config::DEFAULT_ACCOUNT);
    if !matches {
      eprintln!("juctl: jud at {api_url} belongs to another account; use a separate --daemon-url");
      return false;
    }
  }
  let response = client::request(api_url, "/v1/shutdown", json!({}), Duration::from_secs(2));
  let Ok(response) = response else {
    return false;
  };
  if !response.get("ok").and_then(Value::as_bool).unwrap_or(false) {
    return false;
  }
  let pid = response
    .get("pid")
    .and_then(Value::as_i64)
    .map(|pid| pid as i32);
  println!(
    "shutdown requested via API{}",
    pid.map(|pid| format!(" pid={pid}")).unwrap_or_default()
  );
  let deadline = Instant::now() + timeout;
  while Instant::now() < deadline {
    let api_down = !client::health(api_url);
    if let Some(pid) = pid {
      let state = proc_state(pid);
      if api_down && (state.is_empty() || state == "Z") {
        return true;
      }
    } else if api_down {
      return true;
    }
    thread::sleep(Duration::from_millis(100));
  }
  false
}

fn stop_processes(timeout: Duration, force: bool, account: &str) -> i32 {
  let processes = daemon_processes(account);
  if processes.is_empty() {
    println!("jud is not running");
    return 0;
  }
  let pids = processes.iter().map(|(pid, _)| *pid).collect::<Vec<_>>();
  for pid in &pids {
    unsafe {
      libc::kill(*pid, libc::SIGINT);
    }
  }
  let mut remaining = wait_until_stopped(&pids, timeout);
  if !remaining.is_empty() && force {
    for pid in &remaining {
      unsafe {
        libc::kill(*pid, libc::SIGTERM);
      }
    }
    remaining = wait_until_stopped(&remaining, timeout.min(Duration::from_secs(5)));
  }
  if !remaining.is_empty() {
    eprintln!(
      "jud did not stop: {}",
      remaining
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
    );
    return 1;
  }
  println!(
    "stopped jud: {}",
    pids
      .iter()
      .map(ToString::to_string)
      .collect::<Vec<_>>()
      .join(", ")
  );
  0
}

fn command_status(args: &StatusArgs, account: &str) -> i32 {
  let health = client::health_payload(&args.common.daemon_url);
  let health_ok = health
    .as_ref()
    .map(|payload| {
      let actual = payload.get("account").and_then(Value::as_str);
      let matches = actual
        .map(|value| value == account)
        .unwrap_or(account == config::DEFAULT_ACCOUNT);
      if !matches {
        eprintln!(
          "juctl: jud at {} belongs to another account; use a separate --daemon-url",
          args.common.daemon_url
        );
      }
      payload.get("ok").and_then(Value::as_bool) == Some(true) && matches
    })
    .unwrap_or(false);
  let processes = daemon_processes(account);
  if args.json {
    println!(
      "{}",
      serde_json::to_string_pretty(&json!({
        "ok": health_ok,
        "api_url": args.common.daemon_url,
        "stream_url": args.common.stream_url,
        "health": health,
        "processes": processes.iter().map(|(pid, cmdline)| json!({"pid": pid, "cmdline": cmdline})).collect::<Vec<_>>(),
      }))
      .unwrap()
    );
    return if health_ok || !processes.is_empty() {
      0
    } else {
      1
    };
  }
  if health_ok {
    println!("jud API is running at {}", args.common.daemon_url);
    if let Some(queue) = health
      .as_ref()
      .and_then(|payload| payload.get("heavy_queue"))
    {
      let running = queue.get("running").and_then(Value::as_str).unwrap_or("");
      let queued = queue.get("queued").and_then(Value::as_u64).unwrap_or(0);
      println!(
        "heavy queue: running={} queued={}",
        if running.is_empty() { "none" } else { running },
        queued
      );
    }
  } else {
    println!("jud API is not reachable at {}", args.common.daemon_url);
  }
  for (pid, cmdline) in &processes {
    println!("pid {pid}: {}", cmdline.join(" "));
  }
  if health_ok || !processes.is_empty() {
    0
  } else {
    1
  }
}

fn command_start(args: &CommonArgs, account: &str) -> i32 {
  match client::start_daemon(
    &args.daemon_url,
    &args.stream_url,
    account,
    args.headless(),
    &args.daemon_log,
    Duration::from_secs_f64(args.timeout),
  ) {
    Ok(_) => {
      println!("jud is running at {}", args.daemon_url);
      0
    }
    Err(err) => {
      eprintln!("juctl: {err:#}");
      1
    }
  }
}

fn command_resources(args: &CommonArgs) -> i32 {
  match client::request(
    &args.daemon_url,
    "/v1/resources",
    json!({"timeout": args.timeout, "async": true}),
    Duration::from_secs(10),
  ) {
    Ok(mut payload) => {
      if let Some(job_id) = payload.get("job_id").and_then(Value::as_str) {
        match client::wait_job_result(
          &args.daemon_url,
          job_id,
          Duration::from_secs_f64(args.timeout + 30.0),
          Duration::from_millis(100),
        ) {
          Ok(result) => payload = result,
          Err(err) => {
            eprintln!("juctl: {err:#}");
            return 1;
          }
        }
      }
      if payload.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        0
      } else {
        eprintln!(
          "juctl: {}",
          payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("resource query failed")
        );
        1
      }
    }
    Err(err) => {
      eprintln!("juctl: {err:#}");
      1
    }
  }
}

fn path_from_env(keys: &[&str], default: String) -> PathBuf {
  config::expand_tilde(config::env_string(keys, &default))
}

fn auth_cache_path(account: &str) -> PathBuf {
  let default = config::account_auth_cache(account)
    .unwrap_or_else(|_| config::expand_tilde(config::default_auth_cache()));
  if account == config::DEFAULT_ACCOUNT {
    path_from_env(&["JUD_AUTH_CACHE"], default.to_string_lossy().into_owned())
  } else {
    default
  }
}

fn state_file_path(account: &str) -> PathBuf {
  let default = config::account_state_file(account)
    .unwrap_or_else(|_| config::expand_tilde(config::default_state_file()));
  if account == config::DEFAULT_ACCOUNT {
    path_from_env(&["JUD_STATE_FILE"], default.to_string_lossy().into_owned())
  } else {
    default
  }
}

fn chrome_profile_path(account: &str) -> PathBuf {
  let default = config::account_chrome_profile(account)
    .unwrap_or_else(|_| config::expand_tilde(config::default_chrome_profile()));
  if account == config::DEFAULT_ACCOUNT {
    path_from_env(
      &["JUD_CHROME_PROFILE_DIR"],
      default.to_string_lossy().into_owned(),
    )
  } else {
    default
  }
}

fn remove_file_if_exists(path: &Path) -> bool {
  match fs::remove_file(path) {
    Ok(_) => true,
    Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
    Err(err) => {
      eprintln!("juctl: remove {}: {err}", path.display());
      false
    }
  }
}

fn remove_dir_if_exists(path: &Path) -> bool {
  match fs::remove_dir_all(path) {
    Ok(_) => true,
    Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
    Err(err) => {
      eprintln!("juctl: remove {}: {err}", path.display());
      false
    }
  }
}

fn command_login(args: &LoginArgs, account: &str) -> i32 {
  let was_running =
    client::health_for_account(&args.daemon_url, account) || !daemon_processes(account).is_empty();
  if was_running {
    let stop_args = StopArgs {
      common: CommonArgs {
        daemon_url: args.daemon_url.clone(),
        stream_url: args.stream_url.clone(),
        daemon_log: args.daemon_log.clone(),
        timeout: 10.0,
        headless: true,
        visible: false,
      },
      force: true,
    };
    let stop = command_stop(&stop_args, account);
    if stop != 0 {
      return stop;
    }
  }

  let status = match Command::new(daemon_path())
    .arg("--login")
    .arg("--account")
    .arg(account)
    .arg("--visible")
    .arg("--login-timeout")
    .arg(args.timeout.to_string())
    .status()
  {
    Ok(status) => status,
    Err(err) => {
      eprintln!("juctl: start jud login: {err}");
      return 1;
    }
  };
  if !status.success() {
    eprintln!("juctl: login failed");
    return status.code().unwrap_or(1);
  }
  println!("GitCode login cached for jud");

  if was_running && !args.no_restart {
    match client::start_daemon(
      &args.daemon_url,
      &args.stream_url,
      account,
      true,
      &args.daemon_log,
      Duration::from_secs(20),
    ) {
      Ok(_) => println!("jud restarted at {}", args.daemon_url),
      Err(err) => {
        eprintln!("juctl: restart jud after login: {err:#}");
        return 1;
      }
    }
  }
  0
}

fn command_logout(args: &LogoutArgs, account: &str) -> i32 {
  let stop_args = StopArgs {
    common: args.common.clone(),
    force: args.force,
  };
  let stop = command_stop(&stop_args, account);
  if stop != 0 {
    return stop;
  }

  let mut removed = Vec::new();
  let auth = auth_cache_path(account);
  if remove_file_if_exists(&auth) {
    removed.push(auth.display().to_string());
  }
  let state = state_file_path(account);
  if remove_file_if_exists(&state) {
    removed.push(state.display().to_string());
  }
  if !args.keep_profile {
    let profile = chrome_profile_path(account);
    if remove_dir_if_exists(&profile) {
      removed.push(profile.display().to_string());
    }
  }

  if removed.is_empty() {
    println!("no jud login state found");
  } else {
    println!("removed jud login state:");
    for path in removed {
      println!("  {path}");
    }
  }
  0
}

fn command_stop(args: &StopArgs, account: &str) -> i32 {
  if stop_via_api(
    &args.common.daemon_url,
    account,
    Duration::from_secs_f64(args.common.timeout),
  ) {
    return 0;
  }
  stop_processes(
    Duration::from_secs_f64(args.common.timeout),
    args.force,
    account,
  )
}

fn command_reset(args: &CommonArgs, account: &str) -> i32 {
  if !client::health_for_account(&args.daemon_url, account) {
    eprintln!(
      "juctl: jud is not running at {}; start it with juctl start",
      args.daemon_url
    );
    return 1;
  }
  match client::request(
    &args.daemon_url,
    "/v1/reset",
    json!({"timeout": args.timeout}),
    Duration::from_secs_f64(args.timeout + 120.0),
  ) {
    Ok(payload) => {
      if !payload.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        eprintln!(
          "juctl: {}",
          payload
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("notebook reset failed")
        );
        return 1;
      }
      println!(
        "notebook reset: {}",
        payload.get("href").and_then(Value::as_str).unwrap_or("")
      );
      let count = |path: &str| {
        payload
          .pointer(path)
          .and_then(Value::as_array)
          .map(Vec::len)
          .unwrap_or(0)
      };
      println!(
        "closed: {} kernel(s), {} session(s), {} terminal(s)",
        count("/closed/kernels"),
        count("/closed/sessions"),
        count("/closed/terminals"),
      );
      let remaining = count("/remaining/kernels");
      if remaining > 0 {
        eprintln!("juctl: warning: {remaining} kernel(s) did not stop");
      }
      if let Some(opened) = payload.get("opened") {
        if let Some(id) = opened.get("kernel_id").and_then(Value::as_str) {
          println!("opened kernel: {id}");
        }
        if let Some(id) = opened.get("session_id").and_then(Value::as_str) {
          println!("opened session: {id}");
        }
      }
      if let Some(flag) = payload.get("flag") {
        let name = flag.get("name").and_then(Value::as_str).unwrap_or("flag");
        let set_in = flag
          .get("set_in")
          .and_then(Value::as_array)
          .map(Vec::len)
          .unwrap_or(0);
        println!("flag {name} left in {set_in} kernel(s) before reset");
        match flag.get("cleared").and_then(Value::as_bool) {
          Some(true) => println!("flag check: cleared in the new kernel — reset verified"),
          Some(false) => {
            eprintln!(
              "juctl: flag check FAILED: the flag is still present in the new kernel; the reset did not take effect"
            );
            return 1;
          }
          None => {
            if let Some(err) = flag.get("check_error").and_then(Value::as_str) {
              eprintln!("juctl: warning: flag check inconclusive: {err}");
            } else if set_in == 0 {
              eprintln!(
                "juctl: warning: no kernel accepted the flag before reset; nothing to verify"
              );
            }
          }
        }
      }
      0
    }
    Err(err) => {
      eprintln!("juctl: {err:#}");
      1
    }
  }
}

fn resolve_api_urls(api_url: &mut String, stream_url: &mut String, account: &str) {
  if *api_url == client::default_api_url() {
    if let Ok(value) = config::account_api_url(account) {
      *api_url = value;
    }
  }
  if *stream_url == client::default_stream_url() {
    if let Ok(value) = config::account_stream_url(account) {
      *stream_url = value;
    }
  }
}

fn resolve_common_urls(args: &mut CommonArgs, account: &str) {
  resolve_api_urls(&mut args.daemon_url, &mut args.stream_url, account);
}

fn resolve_login_urls(args: &mut LoginArgs, account: &str) {
  resolve_api_urls(&mut args.daemon_url, &mut args.stream_url, account);
}

fn run(cli: Cli) -> Result<i32> {
  let account = config::validate_account_name(&cli.account)?;
  let mut cli = cli;
  match &mut cli.command {
    CommandKind::Accounts { .. } => {}
    CommandKind::Status(args) => resolve_common_urls(&mut args.common, &account),
    CommandKind::Start(args) | CommandKind::Resources(args) | CommandKind::Reset(args) => {
      resolve_common_urls(args, &account)
    }
    CommandKind::Stop(args) | CommandKind::Restart(args) => {
      resolve_common_urls(&mut args.common, &account)
    }
    CommandKind::Login(args) => resolve_login_urls(args, &account),
    CommandKind::Logout(args) => resolve_common_urls(&mut args.common, &account),
  }
  Ok(match cli.command {
    CommandKind::Accounts { command } => command_accounts(command),
    CommandKind::Status(args) => command_status(&args, &account),
    CommandKind::Start(args) => command_start(&args, &account),
    CommandKind::Stop(args) => command_stop(&args, &account),
    CommandKind::Login(args) => command_login(&args, &account),
    CommandKind::Logout(args) => command_logout(&args, &account),
    CommandKind::Restart(args) => {
      let stop = command_stop(&args, &account);
      if stop != 0 {
        stop
      } else {
        command_start(&args.common, &account)
      }
    }
    CommandKind::Resources(args) => command_resources(&args),
    CommandKind::Reset(args) => command_reset(&args, &account),
  })
}

fn command_accounts(command: AccountsCommand) -> i32 {
  match command {
    AccountsCommand::List => {
      let result: anyhow::Result<()> = (|| {
        for account in config::list_accounts() {
          let path = config::account_auth_cache(&account)?;
          let marker = match CookieAuth::from_cache(&path) {
            Ok(auth) if auth.valid(0.0) => {
              format!("logged in ({} cookies)", auth.cookies.len())
            }
            Ok(auth) => format!("expired or incomplete ({} cookies)", auth.cookies.len()),
            Err(_) => "not logged in".to_string(),
          };
          println!("{account}: {marker} [{}]", path.display());
        }
        Ok(())
      })();
      match result {
        Ok(()) => 0,
        Err(err) => {
          eprintln!("juctl: {err:#}");
          1
        }
      }
    }
  }
}

fn main() {
  let cli = Cli::parse();
  let code = match run(cli) {
    Ok(code) => code,
    Err(err) => {
      eprintln!("juctl: {err:#}");
      1
    }
  };
  std::process::exit(code);
}
