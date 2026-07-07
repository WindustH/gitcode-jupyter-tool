use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use clap::{ArgAction, Parser};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use gitcode_jupyter_tool::client;
use gitcode_jupyter_tool::config;
use serde_json::{Value, json};
use std::fs;
use std::io::{self, Cursor, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tar::{Archive, Builder};

const REMOTE_PREFIX: &str = "jupyter:";

fn default_timeout() -> f64 {
  config::env_f64(&["JUCP_TIMEOUT", "JUPYTER_SH_TIMEOUT"], 180.0)
}

#[derive(Parser)]
#[command(
  name = "jucp",
  about = "Copy between the local filesystem and jupyter: paths."
)]
struct Args {
  #[arg(short = 'R', short_alias = 'r', long, action = ArgAction::SetTrue)]
  recursive: bool,
  source: String,
  destination: String,
  #[arg(long, default_value_t = client::default_api_url())]
  daemon_url: String,
  #[arg(long, default_value_t = client::default_stream_url())]
  stream_url: String,
  #[arg(long, action = ArgAction::SetTrue)]
  start_daemon: bool,
  #[arg(long, default_value_t = 20.0)]
  daemon_start_timeout: f64,
  #[arg(long, default_value_t = default_timeout())]
  timeout: f64,
  #[arg(long, default_value_t = 262144)]
  chunk_size: usize,
}

fn is_remote(spec: &str) -> bool {
  spec.starts_with(REMOTE_PREFIX)
}

fn remote_cwd() -> String {
  std::env::var("JUPYTER_CWD").unwrap_or_else(|_| config::DEFAULT_JUPYTER_CWD.to_string())
}

fn remote_path(spec: &str) -> Result<String> {
  let Some(path) = spec.strip_prefix(REMOTE_PREFIX) else {
    bail!("remote path must start with jupyter:: {spec}");
  };
  let path = if path.is_empty() { "." } else { path };
  if path.starts_with('/') {
    return Ok(path.to_string());
  }
  let base = remote_cwd();
  let base = base.trim_end_matches('/');
  if base.is_empty() || base == "." {
    Ok(path.to_string())
  } else if base == "~" {
    Ok(format!("~/{path}"))
  } else {
    Ok(format!("{base}/{path}"))
  }
}

fn remote_basename(path: &str) -> String {
  let stripped = path.trim_end_matches('/');
  if stripped.is_empty() {
    "download".to_string()
  } else {
    stripped
      .rsplit('/')
      .next()
      .unwrap_or("download")
      .to_string()
  }
}

fn local_destination(source_remote_path: &str, destination: &str) -> PathBuf {
  let local = PathBuf::from(destination);
  if destination.ends_with(std::path::MAIN_SEPARATOR) || local.is_dir() {
    local.join(remote_basename(source_remote_path))
  } else {
    local
  }
}

fn remote_destination(source: &Path, destination: &str) -> Result<String> {
  let remote = remote_path(destination)?;
  if remote.ends_with('/') {
    Ok(
      remote
        + source
          .file_name()
          .and_then(|s| s.to_str())
          .unwrap_or("upload"),
    )
  } else {
    Ok(remote)
  }
}

fn make_directory_archive(source: &Path) -> Result<Vec<u8>> {
  let encoder = GzEncoder::new(Vec::new(), Compression::default());
  let mut builder = Builder::new(encoder);
  for entry in fs::read_dir(source).with_context(|| format!("read {}", source.display()))? {
    let entry = entry?;
    let path = entry.path();
    let name = entry.file_name();
    if path.is_dir() {
      builder.append_dir_all(&name, &path)?;
    } else {
      builder.append_path_with_name(&path, &name)?;
    }
  }
  let encoder = builder.into_inner()?;
  Ok(encoder.finish()?)
}

fn unpack_directory_archive(payload: &[u8], destination: &Path) -> Result<()> {
  fs::create_dir_all(destination)?;
  let decoder = GzDecoder::new(Cursor::new(payload));
  let mut archive = Archive::new(decoder);
  archive.unpack(destination)?;
  Ok(())
}

fn ensure_daemon(args: &Args) -> Result<()> {
  client::ensure_daemon(
    &args.daemon_url,
    args.start_daemon,
    &args.stream_url,
    true,
    &client::default_log(),
    Duration::from_secs_f64(args.daemon_start_timeout),
  )
}

fn upload(args: &Args) -> Result<()> {
  ensure_daemon(args)?;
  let source = PathBuf::from(&args.source);
  if !source.exists() {
    bail!("local source does not exist: {}", source.display());
  }
  let destination = remote_destination(&source, &args.destination)?;
  let (payload, mode, is_archive, label) = if source.is_dir() {
    if !args.recursive {
      bail!(
        "{} is a directory; use -r to copy directories",
        source.display()
      );
    }
    (
      make_directory_archive(&source)?,
      0o755,
      true,
      format!("directory {}", source.display()),
    )
  } else {
    (
      fs::read(&source).with_context(|| format!("read {}", source.display()))?,
      source.metadata()?.permissions().mode() & 0o777,
      false,
      source.display().to_string(),
    )
  };
  let mut result = client::request(
    &args.daemon_url,
    "/v1/upload",
    json!({
        "async": true,
        "destination": destination,
        "content_b64": STANDARD.encode(payload),
        "mode": mode,
        "is_archive": is_archive,
        "timeout": args.timeout,
        "chunk_size": args.chunk_size,
    }),
    Duration::from_secs(10),
  )?;
  if let Some(job_id) = result.get("job_id").and_then(Value::as_str) {
    result = client::wait_job_result(
      &args.daemon_url,
      job_id,
      Duration::from_secs_f64(args.timeout + 30.0),
      Duration::from_millis(100),
    )?;
  }
  if !result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
    bail!(
      "{}",
      result
        .get("stdout")
        .or_else(|| result.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("upload failed")
    );
  }
  eprintln!("uploaded {label} -> jupyter:{destination}");
  Ok(())
}

fn download(args: &Args) -> Result<()> {
  ensure_daemon(args)?;
  let source = remote_path(&args.source)?;
  let destination = local_destination(&source, &args.destination);
  let mut result = client::request(
    &args.daemon_url,
    "/v1/download",
    json!({"async": true, "source": source, "recursive": args.recursive, "timeout": args.timeout}),
    Duration::from_secs(10),
  )?;
  if let Some(job_id) = result.get("job_id").and_then(Value::as_str) {
    result = client::wait_job_result(
      &args.daemon_url,
      job_id,
      Duration::from_secs_f64(args.timeout + 30.0),
      Duration::from_millis(100),
    )?;
  }
  if !result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
    bail!(
      "{}",
      result
        .get("stdout")
        .or_else(|| result.get("error"))
        .and_then(Value::as_str)
        .unwrap_or("download failed")
    );
  }
  let payload_b64 = result
    .get("content_b64")
    .and_then(Value::as_str)
    .unwrap_or("");
  let payload = STANDARD.decode(payload_b64.as_bytes())?;
  if args.recursive {
    unpack_directory_archive(&payload, &destination)?;
  } else {
    if let Some(parent) = destination.parent() {
      fs::create_dir_all(parent)?;
    }
    fs::write(&destination, payload)?;
  }
  eprintln!("downloaded jupyter:{source} -> {}", destination.display());
  Ok(())
}

fn run(args: &Args) -> Result<i32> {
  let source_remote = is_remote(&args.source);
  let destination_remote = is_remote(&args.destination);
  if source_remote == destination_remote {
    bail!("exactly one address must be remote, and remote paths must start with jupyter:");
  }
  if destination_remote {
    upload(args)?;
  } else {
    download(args)?;
  }
  Ok(0)
}

fn main() {
  let args = Args::parse();
  let code = match run(&args) {
    Ok(code) => code,
    Err(err) => {
      let _ = writeln!(io::stderr(), "jucp: {err:#}");
      1
    }
  };
  std::process::exit(code);
}
