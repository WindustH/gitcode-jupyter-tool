use anyhow::{Context, Result, bail};
use clap::{ArgAction, Parser};
use gitcode_jupyter_tool::client;
use gitcode_jupyter_tool::config;
use gitcode_jupyter_tool::util::{
  quote_remote_path_for_shell, shell_quote, strip_terminal_noise, token_hex,
};
use serde_json::{Value, json};
use std::fs;
use std::io::ErrorKind;
use std::io::{self, IsTerminal, Read, Write};
use std::net::TcpStream;
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

fn default_timeout() -> f64 {
  config::env_f64(&["JUSH_TIMEOUT", "JUPYTER_SH_TIMEOUT"], 180.0)
}

#[derive(Parser)]
#[command(
  name = "jush",
  about = "Run a bash-like shell through gjtd.",
  trailing_var_arg = true,
  allow_hyphen_values = true
)]
struct Args {
  #[arg(short = 'c', value_name = "command")]
  command_string: Option<String>,
  #[arg(short = 's', action = ArgAction::SetTrue)]
  read_stdin: bool,
  #[arg(short = 'i', action = ArgAction::SetTrue)]
  interactive: bool,
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
  #[arg(long, default_value_t = 0)]
  rows: u16,
  #[arg(long, default_value_t = 0)]
  cols: u16,
  #[arg(long, action = ArgAction::SetTrue)]
  raw: bool,
  #[arg(long, action = ArgAction::SetTrue)]
  no_prelude: bool,
  #[arg(value_name = "argument")]
  shell_args: Vec<String>,
}

fn terminal_size(args: &Args) -> (u16, u16) {
  let fallback = (args.rows.max(24), args.cols.max(120));
  if args.rows != 0 && args.cols != 0 {
    return (args.rows, args.cols);
  }
  let mut size = libc::winsize {
    ws_row: 0,
    ws_col: 0,
    ws_xpixel: 0,
    ws_ypixel: 0,
  };
  let ok = unsafe { libc::ioctl(1, libc::TIOCGWINSZ, &mut size) } == 0;
  if ok {
    (
      if args.rows == 0 {
        size.ws_row.max(1)
      } else {
        args.rows
      },
      if args.cols == 0 {
        size.ws_col.max(1)
      } else {
        args.cols
      },
    )
  } else {
    fallback
  }
}

fn remote_cwd() -> String {
  std::env::var("JUPYTER_CWD").unwrap_or_else(|_| config::DEFAULT_JUPYTER_CWD.to_string())
}

fn with_remote_cwd(command: String) -> String {
  format!(
    "cd {}\n{}",
    quote_remote_path_for_shell(&remote_cwd()),
    command
  )
}

fn build_c_command(command: &str, command_args: &[String]) -> String {
  let mut quoted = vec![shell_quote(command)];
  quoted.extend(command_args.iter().map(|arg| shell_quote(arg)));
  format!("bash -c {}", quoted.join(" "))
}

fn build_stdin_script_command(text: &str, script_args: &[String]) -> String {
  let mut text = text.to_string();
  if !text.ends_with('\n') {
    text.push('\n');
  }
  let mut delimiter = format!("__JUSH_STDIN_{}__", token_hex(12));
  while text.contains(&delimiter) {
    delimiter = format!("__JUSH_STDIN_{}__", token_hex(12));
  }
  let quoted_args = script_args
    .iter()
    .map(|arg| shell_quote(arg))
    .collect::<Vec<_>>()
    .join(" ");
  let runner = if quoted_args.is_empty() {
    "bash -s".to_string()
  } else {
    format!("bash -s -- {quoted_args}")
  };
  format!(
    "{runner} <<'{delimiter}'\n{}{delimiter}\n",
    text.trim_end_matches('\n')
  )
}

fn build_script_command(path: &PathBuf, script_args: &[String]) -> Result<String> {
  let raw = fs::read(path).with_context(|| format!("read {}", path.display()))?;
  let mut text = String::from_utf8_lossy(&raw).into_owned();
  if !text.ends_with('\n') {
    text.push('\n');
  }
  let mut delimiter = format!("__JUSH_SCRIPT_{}__", token_hex(12));
  while text.contains(&delimiter) {
    delimiter = format!("__JUSH_SCRIPT_{}__", token_hex(12));
  }
  let remote_name = path
    .file_name()
    .and_then(|s| s.to_str())
    .unwrap_or("script");
  let remote_path = format!("/tmp/jush-{}-{}", token_hex(8), remote_name);
  let quoted_args = script_args
    .iter()
    .map(|arg| shell_quote(arg))
    .collect::<Vec<_>>()
    .join(" ");
  let executable = raw.starts_with(b"#!")
    || path
      .metadata()
      .map(|m| m.permissions().mode() & 0o111 != 0)
      .unwrap_or(false);
  let mut runner = if executable {
    "\"$remote_script\"".to_string()
  } else {
    "bash \"$remote_script\"".to_string()
  };
  if !quoted_args.is_empty() {
    runner.push(' ');
    runner.push_str(&quoted_args);
  }
  Ok(
    [
      format!("remote_script={}", shell_quote(&remote_path)),
      "cleanup_jush_script() { rm -f \"$remote_script\"; }".to_string(),
      "trap cleanup_jush_script EXIT".to_string(),
      format!("cat > \"$remote_script\" <<'{delimiter}'"),
      text.trim_end_matches('\n').to_string(),
      delimiter,
      "chmod +x \"$remote_script\"".to_string(),
      runner,
      String::new(),
    ]
    .join("\n"),
  )
}

fn command_from_args(args: &Args) -> Result<Option<String>> {
  let mut shell_args = args.shell_args.clone();
  if shell_args.first().is_some_and(|value| value == "--") {
    shell_args.remove(0);
  }
  if let Some(command) = &args.command_string {
    return Ok(Some(with_remote_cwd(build_c_command(command, &shell_args))));
  }
  if args.read_stdin {
    let mut text = String::new();
    io::stdin().read_to_string(&mut text)?;
    return Ok(Some(with_remote_cwd(build_stdin_script_command(
      &text,
      &shell_args,
    ))));
  }
  if let Some(first) = shell_args.first() {
    let path = PathBuf::from(first);
    if !path.exists() {
      bail!("{}: No such file or directory", path.display());
    }
    return Ok(Some(with_remote_cwd(build_script_command(
      &path,
      &shell_args[1..],
    )?)));
  }
  if args.interactive {
    return Ok(None);
  }
  if !io::stdin().is_terminal() {
    let mut text = String::new();
    io::stdin().read_to_string(&mut text)?;
    return Ok(Some(with_remote_cwd(build_stdin_script_command(
      &text,
      &[],
    ))));
  }
  Ok(None)
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

fn run_command(command: String, args: &Args) -> Result<i32> {
  ensure_daemon(args)?;
  let (rows, cols) = terminal_size(args);
  let mut result = client::request(
    &args.daemon_url,
    "/v1/run",
    json!({
        "async": true,
        "command": command,
        "timeout": args.timeout,
        "rows": rows,
        "cols": cols,
        "raw": args.raw,
        "no_prelude": args.no_prelude,
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
  let output = result.get("stdout").and_then(Value::as_str).unwrap_or("");
  if args.raw {
    print!("{output}");
  } else {
    print!("{}", strip_terminal_noise(output));
  }
  io::stdout().flush().ok();
  if !result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
    if let Some(error) = result.get("error").and_then(Value::as_str) {
      eprintln!("jush: {error}");
    }
  }
  Ok(
    result
      .get("exit_code")
      .and_then(Value::as_i64)
      .unwrap_or_else(|| {
        if result.get("ok").and_then(Value::as_bool).unwrap_or(false) {
          0
        } else {
          1
        }
      }) as i32,
  )
}

fn read_stream_header(sock: &mut TcpStream, timeout: Duration) -> Result<(Value, Vec<u8>)> {
  sock.set_read_timeout(Some(timeout))?;
  let mut data = Vec::new();
  while !data.contains(&b'\n') {
    let mut buf = [0u8; 4096];
    let size = match sock.read(&mut buf) {
      Ok(size) => size,
      Err(err) if matches!(err.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
        bail!(
          "gjtd stream did not become ready within {:.1}s; check /tmp/gjtd.log",
          timeout.as_secs_f64()
        );
      }
      Err(err) => return Err(err.into()),
    };
    if size == 0 {
      bail!("gjtd stream closed before handshake");
    }
    data.extend_from_slice(&buf[..size]);
    if data.len() > 65536 {
      bail!("gjtd stream handshake is too large");
    }
  }
  sock.set_read_timeout(None)?;
  let index = data.iter().position(|b| *b == b'\n').unwrap();
  let header = serde_json::from_slice(&data[..index])?;
  Ok((header, data[index + 1..].to_vec()))
}

fn interactive(args: &Args) -> Result<i32> {
  ensure_daemon(args)?;
  let (rows, cols) = terminal_size(args);
  eprintln!("Opening remote interactive shell through gjtd...");
  let mut sock = client::connect_tcp(
    &args.stream_url,
    Duration::from_secs_f64(args.daemon_start_timeout),
  )?;
  sock.write_all(serde_json::to_string(&json!({"rows": rows, "cols": cols}))?.as_bytes())?;
  sock.write_all(b"\n")?;
  let (start, initial_output) = read_stream_header(
    &mut sock,
    Duration::from_secs_f64(args.daemon_start_timeout),
  )?;
  if !start.get("ok").and_then(Value::as_bool).unwrap_or(false) {
    bail!(
      "{}",
      start
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or("gjtd stream failed")
    );
  }
  eprintln!(
    "Connected to {} through gjtd stream.",
    start
      .get("href")
      .and_then(Value::as_str)
      .unwrap_or(&args.daemon_url)
  );
  eprintln!("Press Ctrl-D to exit the remote shell; Ctrl-] force-disconnects locally.");
  if !initial_output.is_empty() {
    io::stdout().write_all(&initial_output)?;
  }
  if remote_cwd() != config::DEFAULT_JUPYTER_CWD {
    sock.write_all(format!("cd {}\n", quote_remote_path_for_shell(&remote_cwd())).as_bytes())?;
  }
  raw_terminal_loop(sock)?;
  Ok(0)
}

fn raw_terminal_loop(mut sock: TcpStream) -> Result<()> {
  let stdin_fd = io::stdin().as_raw_fd();
  let sock_fd = sock.as_raw_fd();
  let old_termios = if io::stdin().is_terminal() {
    let mut term = unsafe { std::mem::zeroed::<libc::termios>() };
    if unsafe { libc::tcgetattr(stdin_fd, &mut term) } == 0 {
      let old = term;
      unsafe { libc::cfmakeraw(&mut term) };
      unsafe { libc::tcsetattr(stdin_fd, libc::TCSADRAIN, &term) };
      Some(old)
    } else {
      None
    }
  } else {
    None
  };
  let result = (|| {
    loop {
      let mut fds = [
        libc::pollfd {
          fd: stdin_fd,
          events: libc::POLLIN,
          revents: 0,
        },
        libc::pollfd {
          fd: sock_fd,
          events: libc::POLLIN,
          revents: 0,
        },
      ];
      let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, 50) };
      if ready < 0 {
        bail!("poll failed");
      }
      if fds[0].revents & libc::POLLIN != 0 {
        let mut buf = [0u8; 4096];
        let size = unsafe { libc::read(stdin_fd, buf.as_mut_ptr().cast(), buf.len()) };
        if size <= 0 {
          break;
        }
        let data = &buf[..size as usize];
        if data == b"\x1d" {
          break;
        }
        sock.write_all(data)?;
      }
      if fds[1].revents & libc::POLLIN != 0 {
        let mut buf = [0u8; 65536];
        let size = sock.read(&mut buf)?;
        if size == 0 {
          break;
        }
        io::stdout().write_all(&buf[..size])?;
        io::stdout().flush().ok();
      }
    }
    Ok::<_, anyhow::Error>(())
  })();
  if let Some(old) = old_termios {
    unsafe { libc::tcsetattr(stdin_fd, libc::TCSADRAIN, &old) };
  }
  result
}

fn main() {
  let args = Args::parse();
  let code = match command_from_args(&args).and_then(|command| match command {
    Some(command) => run_command(command, &args),
    None => interactive(&args),
  }) {
    Ok(code) => code,
    Err(err) => {
      eprintln!("jush: {err:#}");
      1
    }
  };
  std::process::exit(code);
}
