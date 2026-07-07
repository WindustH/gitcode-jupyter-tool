use std::env;
use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "gitcode-jupyter-tool";
pub const DEFAULT_CONFIG_DIR: &str = "/home/windy/.config/gitcode-jupyter-tool";
pub const DEFAULT_HUB_URL: &str = "https://gitcode.com/cann/cann-learning-hub";
pub const DEFAULT_REPO_URL: &str = "https://gitcode.com/cann/cann-learning-hub.git";
pub const DEFAULT_NOTEBOOK_PATH: &str = "quick_start/cann_basics";
pub const DEFAULT_SCAN_FILE_PATH: &str = "quick_start/cann_basics/01_ai_basics.ipynb";
pub const DEFAULT_API_URL: &str = "http://127.0.0.1:18787";
pub const DEFAULT_STREAM_URL: &str = "tcp://127.0.0.1:18788";
pub const DEFAULT_LOG: &str = "/tmp/gjtd.log";
pub const DEFAULT_STATE_FILE: &str = "/home/windy/.config/gitcode-jupyter-tool/state.json";
pub const DEFAULT_CDP_LIST_URL: &str = "http://127.0.0.1:9222/json";
pub const DEFAULT_LISTEN_HOST: &str = "127.0.0.1";
pub const DEFAULT_LISTEN_PORT: u16 = 18787;
pub const DEFAULT_STREAM_HOST: &str = "127.0.0.1";
pub const DEFAULT_STREAM_PORT: u16 = 18788;
pub const DEFAULT_JUPYTER_CWD: &str = "~";

pub fn default_chrome_profile() -> String {
  format!("{DEFAULT_CONFIG_DIR}/chrome-profile")
}

pub fn default_auth_cache() -> String {
  format!("{DEFAULT_CONFIG_DIR}/auth.json")
}

pub fn default_chrome_bin() -> String {
  if let Ok(value) = env::var("CHROME") {
    if !value.is_empty() {
      return value;
    }
  }
  if Path::new("/opt/google/chrome/google-chrome").exists() {
    return "/opt/google/chrome/google-chrome".to_string();
  }
  "google-chrome-stable".to_string()
}

pub fn env_string(keys: &[&str], default: &str) -> String {
  for key in keys {
    if let Ok(value) = env::var(key) {
      if !value.is_empty() {
        return value;
      }
    }
  }
  default.to_string()
}

pub fn env_f64(keys: &[&str], default: f64) -> f64 {
  for key in keys {
    if let Ok(value) = env::var(key) {
      if let Ok(parsed) = value.parse::<f64>() {
        return parsed;
      }
    }
  }
  default
}

pub fn env_u16(keys: &[&str], default: u16) -> u16 {
  for key in keys {
    if let Ok(value) = env::var(key) {
      if let Ok(parsed) = value.parse::<u16>() {
        return parsed;
      }
    }
  }
  default
}

pub fn env_u64(keys: &[&str], default: u64) -> u64 {
  for key in keys {
    if let Ok(value) = env::var(key) {
      if let Ok(parsed) = value.parse::<u64>() {
        return parsed;
      }
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
  env::var_os("HOME")
    .map(PathBuf::from)
    .unwrap_or_else(|| PathBuf::from("/home/windy"))
}
