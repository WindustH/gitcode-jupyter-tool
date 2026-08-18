use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{ErrorKind, Write};
use std::net::TcpListener;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "gitcode-jupyter-tool";
pub const DEFAULT_ACCOUNT: &str = "default";
pub const DEFAULT_HUB_URL: &str = "https://gitcode.com/cann/cann-learning-hub";
pub const DEFAULT_REPO_URL: &str = "https://gitcode.com/cann/cann-learning-hub.git";
pub const DEFAULT_NOTEBOOK_PATH: &str = "quick_start/cann_basics";
pub const DEFAULT_SCAN_FILE_PATH: &str = "quick_start/cann_basics/01_ai_basics.ipynb";
pub const DEFAULT_API_URL: &str = "http://127.0.0.1:61000";
pub const DEFAULT_STREAM_URL: &str = "tcp://127.0.0.1:61001";
pub const DEFAULT_LOG: &str = "/tmp/jud.log";
pub const DEFAULT_LISTEN_HOST: &str = "127.0.0.1";
pub const DEFAULT_LISTEN_PORT: u16 = PORT_POOL_START;
pub const DEFAULT_STREAM_HOST: &str = "127.0.0.1";
pub const DEFAULT_STREAM_PORT: u16 = PORT_POOL_START + 1;
pub const DEFAULT_JUPYTER_CWD: &str = "~";
pub const PORT_POOL_START: u16 = 61000;
pub const PORT_POOL_END: u16 = 61199;
pub const CDP_PORT_POOL_START: u16 = 61800;
pub const CDP_PORT_POOL_END: u16 = 61999;

pub fn default_account() -> String {
  env::var("JUD_ACCOUNT").unwrap_or_else(|_| DEFAULT_ACCOUNT.to_string())
}

pub fn validate_account_name(value: &str) -> anyhow::Result<String> {
  let value = value.trim();
  if value.is_empty() || value == "." || value == ".." || value.len() > 64 {
    anyhow::bail!("invalid account name: {value:?}");
  }
  if !value
    .bytes()
    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
  {
    anyhow::bail!(
      "invalid account name {value:?}; use only ASCII letters, digits, '-', '_' or '.'"
    );
  }
  Ok(value.to_string())
}

pub fn default_config_dir() -> String {
  for key in ["JUD_CONFIG_DIR"] {
    if let Ok(value) = env::var(key)
      && !value.is_empty()
    {
      return value;
    }
  }

  if let Ok(value) = env::var("XDG_CONFIG_HOME")
    && !value.is_empty()
  {
    return PathBuf::from(value)
      .join(APP_NAME)
      .to_string_lossy()
      .into_owned();
  }

  home_dir()
    .join(".config")
    .join(APP_NAME)
    .to_string_lossy()
    .into_owned()
}

pub fn default_cache_dir() -> String {
  if let Ok(value) = env::var("JUD_CACHE_DIR")
    && !value.is_empty()
  {
    return value;
  }
  if let Ok(value) = env::var("XDG_CACHE_HOME")
    && !value.is_empty()
  {
    return PathBuf::from(value)
      .join(APP_NAME)
      .to_string_lossy()
      .into_owned();
  }
  home_dir()
    .join(".cache")
    .join(APP_NAME)
    .to_string_lossy()
    .into_owned()
}

pub fn default_chrome_profile() -> String {
  account_chrome_profile(&default_account())
    .unwrap_or_else(|_| PathBuf::from(default_cache_dir()).join("chrome-profile"))
    .to_string_lossy()
    .into_owned()
}

pub fn default_auth_cache() -> String {
  account_auth_cache(&default_account())
    .unwrap_or_else(|_| PathBuf::from(default_config_dir()).join("auth.json"))
    .to_string_lossy()
    .into_owned()
}

pub fn default_state_file() -> String {
  account_state_file(&default_account())
    .unwrap_or_else(|_| PathBuf::from(default_config_dir()).join("state.json"))
    .to_string_lossy()
    .into_owned()
}

fn accounts_dir() -> PathBuf {
  PathBuf::from(default_config_dir()).join("accounts")
}

pub fn account_auth_cache(account: &str) -> anyhow::Result<PathBuf> {
  let account = validate_account_name(account)?;
  if account == DEFAULT_ACCOUNT {
    Ok(PathBuf::from(default_config_dir()).join("auth.json"))
  } else {
    Ok(accounts_dir().join(format!("{account}.json")))
  }
}

pub fn account_state_file(account: &str) -> anyhow::Result<PathBuf> {
  let account = validate_account_name(account)?;
  if account == DEFAULT_ACCOUNT {
    Ok(PathBuf::from(default_config_dir()).join("state.json"))
  } else {
    Ok(accounts_dir().join(account).join("state.json"))
  }
}

pub fn account_chrome_profile(account: &str) -> anyhow::Result<PathBuf> {
  let account = validate_account_name(account)?;
  let cache_dir = PathBuf::from(default_cache_dir());
  if account == DEFAULT_ACCOUNT {
    Ok(cache_dir.join("chrome-profile"))
  } else {
    Ok(
      cache_dir
        .join("accounts")
        .join(account)
        .join("chrome-profile"),
    )
  }
}

pub fn account_cdp_port(account: &str) -> anyhow::Result<u16> {
  let account = validate_account_name(account)?;
  if let Ok(value) = env::var("JUD_CDP_PORT")
    && !value.is_empty()
  {
    return Ok(value.parse().unwrap_or(CDP_PORT_POOL_START));
  }
  let path = accounts_dir().join(&account).join("cdp-port");
  if let Some(port) = read_port_file(&path, CDP_PORT_POOL_START..=CDP_PORT_POOL_END) {
    return Ok(port);
  }
  let port = find_available_port(CDP_PORT_POOL_START..=CDP_PORT_POOL_END, &account)?;
  write_port_file(&path, port)?;
  Ok(read_port_file(&path, CDP_PORT_POOL_START..=CDP_PORT_POOL_END).unwrap_or(port))
}

pub fn account_listen_port(account: &str) -> anyhow::Result<u16> {
  let account = validate_account_name(account)?;
  if env::var("JUD_LISTEN_PORT").is_ok() {
    return Ok(env_u16(&["JUD_LISTEN_PORT"], PORT_POOL_START));
  }
  Ok(account_endpoint_ports(&account)?.0)
}

pub fn account_stream_port(account: &str) -> anyhow::Result<u16> {
  let account = validate_account_name(account)?;
  if env::var("JUD_STREAM_PORT").is_ok() {
    return Ok(env_u16(&["JUD_STREAM_PORT"], PORT_POOL_START + 1));
  }
  Ok(account_endpoint_ports(&account)?.1)
}

pub fn account_api_url(account: &str) -> anyhow::Result<String> {
  let account = validate_account_name(account)?;
  if let Ok(value) = env::var("JUD_API_URL")
    && !value.is_empty()
  {
    return Ok(value);
  }
  Ok(format!(
    "http://127.0.0.1:{}",
    account_listen_port(&account)?
  ))
}

pub fn account_stream_url(account: &str) -> anyhow::Result<String> {
  let account = validate_account_name(account)?;
  if let Ok(value) = env::var("JUD_STREAM_URL")
    && !value.is_empty()
  {
    return Ok(value);
  }
  Ok(format!(
    "tcp://127.0.0.1:{}",
    account_stream_port(&account)?
  ))
}

fn endpoint_file(account: &str) -> PathBuf {
  accounts_dir().join(account).join("ports")
}

fn account_endpoint_ports(account: &str) -> anyhow::Result<(u16, u16)> {
  let path = endpoint_file(account);
  let pool = PORT_POOL_START..=PORT_POOL_END;
  if let Some((api, stream)) = read_pair_file(&path, pool.clone()) {
    return Ok((api, stream));
  }
  let (api, stream) = find_available_pair(pool, account)?;
  write_pair_file(&path, api, stream)?;
  Ok(read_pair_file(&path, PORT_POOL_START..=PORT_POOL_END).unwrap_or((api, stream)))
}

fn read_port_file(path: &Path, range: RangeInclusive<u16>) -> Option<u16> {
  let port = fs::read_to_string(path).ok()?.trim().parse().ok()?;
  range.contains(&port).then_some(port)
}

fn read_pair_file(path: &Path, range: RangeInclusive<u16>) -> Option<(u16, u16)> {
  let contents = fs::read_to_string(path).ok()?;
  let mut values = contents.split_whitespace().map(str::parse::<u16>);
  let api = values.next()?.ok()?;
  let stream = values.next()?.ok()?;
  (api != stream && range.contains(&api) && range.contains(&stream)).then_some((api, stream))
}

fn write_port_file(path: &Path, port: u16) -> anyhow::Result<()> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent)?;
  }
  match fs::OpenOptions::new()
    .write(true)
    .create_new(true)
    .open(path)
  {
    Ok(mut file) => {
      writeln!(file, "{port}")?;
      file.sync_all()?;
    }
    Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
    Err(err) => return Err(err.into()),
  }
  Ok(())
}

fn write_pair_file(path: &Path, api: u16, stream: u16) -> anyhow::Result<()> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent)?;
  }
  match fs::OpenOptions::new()
    .write(true)
    .create_new(true)
    .open(path)
  {
    Ok(mut file) => {
      writeln!(file, "{api} {stream}")?;
      file.sync_all()?;
    }
    Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
    Err(err) => return Err(err.into()),
  }
  Ok(())
}

fn find_available_port(range: RangeInclusive<u16>, account: &str) -> anyhow::Result<u16> {
  let ports = ordered_ports(range, account);
  for port in ports {
    if TcpListener::bind(("127.0.0.1", port)).is_ok() {
      return Ok(port);
    }
  }
  anyhow::bail!("no free JUD port in the configured port pool")
}

fn find_available_pair(range: RangeInclusive<u16>, account: &str) -> anyhow::Result<(u16, u16)> {
  let ports = ordered_ports(range, account);
  for window in ports.windows(2) {
    let [api, stream] = window else { continue };
    if api.abs_diff(*stream) != 1 {
      continue;
    }
    let Ok(api_listener) = TcpListener::bind(("127.0.0.1", *api)) else {
      continue;
    };
    if TcpListener::bind(("127.0.0.1", *stream)).is_ok() {
      drop(api_listener);
      return Ok((*api, *stream));
    }
  }
  anyhow::bail!("no free JUD API/stream port pair in the configured port pool")
}

fn ordered_ports(range: RangeInclusive<u16>, account: &str) -> Vec<u16> {
  let start = *range.start();
  let len = range.clone().count();
  let mut hasher = std::collections::hash_map::DefaultHasher::new();
  account.hash(&mut hasher);
  let offset = (hasher.finish() as usize) % len;
  (0..len)
    .map(|index| start + ((offset + index) % len) as u16)
    .collect()
}

pub fn list_accounts() -> Vec<String> {
  let mut accounts = vec![DEFAULT_ACCOUNT.to_string()];
  if let Ok(entries) = fs::read_dir(accounts_dir()) {
    for entry in entries.flatten() {
      let path = entry.path();
      if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
        continue;
      }
      if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str())
        && validate_account_name(stem).is_ok()
        && stem != DEFAULT_ACCOUNT
      {
        accounts.push(stem.to_string());
      }
    }
  }
  accounts.sort();
  accounts.dedup();
  accounts
}

pub fn default_chrome_bin() -> String {
  if let Ok(value) = env::var("CHROME")
    && !value.is_empty()
  {
    return value;
  }
  if Path::new("/opt/google/chrome/google-chrome").exists() {
    return "/opt/google/chrome/google-chrome".to_string();
  }
  "google-chrome-stable".to_string()
}

pub fn env_string(keys: &[&str], default: &str) -> String {
  for key in keys {
    if let Ok(value) = env::var(key)
      && !value.is_empty()
    {
      return value;
    }
  }
  default.to_string()
}

pub fn env_f64(keys: &[&str], default: f64) -> f64 {
  for key in keys {
    if let Ok(value) = env::var(key)
      && let Ok(parsed) = value.parse::<f64>()
    {
      return parsed;
    }
  }
  default
}

pub fn env_u16(keys: &[&str], default: u16) -> u16 {
  for key in keys {
    if let Ok(value) = env::var(key)
      && let Ok(parsed) = value.parse::<u16>()
    {
      return parsed;
    }
  }
  default
}

pub fn env_u64(keys: &[&str], default: u64) -> u64 {
  for key in keys {
    if let Ok(value) = env::var(key)
      && let Ok(parsed) = value.parse::<u64>()
    {
      return parsed;
    }
  }
  default
}

pub fn expand_tilde(path: impl AsRef<str>) -> PathBuf {
  let path = path.as_ref();
  if path == "~" {
    return home_dir();
  }
  if let Some(rest) = path.strip_prefix("~/") {
    return home_dir().join(rest);
  }
  PathBuf::from(path)
}

pub fn home_dir() -> PathBuf {
  if let Some(home) = env::var_os("HOME").filter(|value| !value.is_empty()) {
    return PathBuf::from(home);
  }
  if let Some(home) = passwd_home_dir() {
    return home;
  }
  PathBuf::from("/")
}

/// Resolve the home directory from passwd when HOME is unset or empty
/// (e.g. under systemd or stripped environments), so config/cache paths
/// never resolve relative to the current working directory.
fn passwd_home_dir() -> Option<PathBuf> {
  unsafe {
    let pw = libc::getpwuid(libc::getuid());
    if pw.is_null() {
      return None;
    }
    let dir = std::ffi::CStr::from_ptr((*pw).pw_dir)
      .to_string_lossy()
      .into_owned();
    (!dir.is_empty()).then(|| PathBuf::from(dir))
  }
}
