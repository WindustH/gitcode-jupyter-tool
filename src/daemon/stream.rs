use super::Service;
use super::terminal::{prepare_stream_prompt, terminal_rows_cols};
use crate::direct::ApiTerminal;
use crate::util::{log, token_hex};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

pub(super) fn run_stream_server(service: Arc<Service>, stop: Arc<AtomicBool>) -> Result<()> {
  let args = &service.context.args;
  let listener = TcpListener::bind(format!("{}:{}", args.stream_host, args.stream_port))?;
  listener.set_nonblocking(true)?;
  log(format!(
    "gjtd terminal stream listening on tcp://{}:{}",
    args.stream_host, args.stream_port
  ));
  while !stop.load(Ordering::Relaxed) {
    match listener.accept() {
      Ok((stream, _)) => {
        let service = Arc::clone(&service);
        thread::spawn(move || {
          let _ = handle_stream(service, stream);
        });
      }
      Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
        thread::sleep(Duration::from_millis(50))
      }
      Err(err) => return Err(err.into()),
    }
  }
  Ok(())
}

fn read_stream_header(stream: &mut TcpStream) -> Result<(Value, Vec<u8>)> {
  let mut data = Vec::new();
  while !data.contains(&b'\n') {
    let mut buf = [0u8; 4096];
    let size = stream.read(&mut buf)?;
    if size == 0 {
      bail!("stream client closed before header");
    }
    data.extend_from_slice(&buf[..size]);
    if data.len() > 65536 {
      bail!("stream header is too large");
    }
  }
  let index = data.iter().position(|b| *b == b'\n').unwrap();
  let line = &data[..index];
  let rest = data[index + 1..].to_vec();
  Ok((serde_json::from_slice(line)?, rest))
}

fn handle_stream(service: Arc<Service>, mut stream: TcpStream) -> Result<()> {
  let terminal = (|| {
    let (request, initial_input) = read_stream_header(&mut stream)?;
    let (rows, cols) = terminal_rows_cols(
      request.get("rows").and_then(Value::as_u64),
      request.get("cols").and_then(Value::as_u64),
    );
    log(format!(
      "stream client connected; opening interactive terminal rows={rows} cols={cols}"
    ));
    let context = service.direct_context(false)?;
    let href = context.notebook.lab_url.clone();
    let mut terminal = ApiTerminal::new(context.client, context.notebook, rows, cols);
    terminal.start(Duration::from_secs(30))?;
    log(format!("stream terminal opened: {href}"));
    stream.write_all(
      serde_json::to_string(&json!({"ok": true, "id": token_hex(12), "href": href}))?.as_bytes(),
    )?;
    stream.write_all(b"\n")?;
    match prepare_stream_prompt(&mut terminal) {
      Ok(initial_output) if !initial_output.is_empty() => {
        stream.write_all(initial_output.as_bytes())?;
      }
      Ok(_) => {}
      Err(err) => {
        let _ = stream.write_all(format!("\n[gjtd: failed to prepare prompt: {err}]\n").as_bytes());
      }
    }
    if !initial_input.is_empty() {
      terminal.send(&String::from_utf8_lossy(&initial_input))?;
    }
    Ok::<ApiTerminal, anyhow::Error>(terminal)
  })();

  let mut terminal = match terminal {
    Ok(terminal) => terminal,
    Err(err) => {
      let _ = stream.write_all(
        serde_json::to_string(&json!({"ok": false, "error": err.to_string()}))?.as_bytes(),
      );
      let _ = stream.write_all(b"\n");
      return Ok(());
    }
  };

  stream.set_nonblocking(true)?;
  let mut buf = [0u8; 65536];
  while !terminal.closed {
    match stream.read(&mut buf) {
      Ok(0) => break,
      Ok(size) => terminal.send(&String::from_utf8_lossy(&buf[..size]))?,
      Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
      Err(err) => return Err(err.into()),
    }
    let output = terminal.read_once(Duration::from_millis(20))?;
    if !output.is_empty() {
      stream.write_all(output.as_bytes())?;
    }
  }
  terminal.close();
  Ok(())
}
