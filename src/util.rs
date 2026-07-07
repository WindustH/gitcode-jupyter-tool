use anyhow::{Context, Result};
use chrono::Local;
use once_cell::sync::Lazy;
use rand::RngCore;
use regex::Regex;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

static ANSI_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\x1b\[[0-9;?]*[ -/]*[@-~]").unwrap());

pub fn log(message: impl AsRef<str>) {
  let timestamp = Local::now().format("%Y-%m-%d %H:%M:%S");
  println!("[{timestamp}] {}", message.as_ref());
}

pub fn eprintln_line(message: impl AsRef<str>) {
  eprintln!("{}", message.as_ref());
}

pub fn token_hex(bytes: usize) -> String {
  let mut data = vec![0u8; bytes];
  rand::thread_rng().fill_bytes(&mut data);
  data.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn strip_terminal_noise(text: &str) -> String {
  ANSI_RE.replace_all(text, "").replace('\r', "")
}

pub fn shell_quote(value: &str) -> String {
  if value.is_empty() {
    return "''".to_string();
  }
  if value
        .bytes()
        .all(|b| matches!(b, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'))
    {
        return value.to_string();
    }
  format!("'{}'", value.replace('\'', "'\"'\"'"))
}

pub fn quote_remote_path_for_shell(path: &str) -> String {
  if path == "~" {
    "~".to_string()
  } else if let Some(rest) = path.strip_prefix("~/") {
    format!("~/{}", shell_quote(rest))
  } else {
    shell_quote(path)
  }
}

pub fn write_atomic_0600(path: &Path, bytes: &[u8]) -> Result<()> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
  }
  let tmp = path.with_file_name(format!(
    "{}.tmp-{}",
    path.file_name().and_then(|s| s.to_str()).unwrap_or("tmp"),
    token_hex(4)
  ));
  {
    let mut file = fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    file.write_all(bytes)?;
    file.flush()?;
  }
  fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).ok();
  fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
  fs::set_permissions(path, fs::Permissions::from_mode(0o600)).ok();
  Ok(())
}

pub fn read_json_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
  let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
  Ok(serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?)
}

pub fn write_json_file(path: &Path, value: &serde_json::Value) -> Result<()> {
  if let Some(parent) = path.parent() {
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
  }
  let text = serde_json::to_string_pretty(value)? + "\n";
  fs::write(path, text).with_context(|| format!("write {}", path.display()))?;
  Ok(())
}
