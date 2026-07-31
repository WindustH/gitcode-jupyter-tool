use super::Service;
use super::terminal::terminal_rows_cols;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;
use tiny_http::{Header, Method, Response, Server, StatusCode};

fn requested_session(body: &Value) -> Option<String> {
  body
    .get("session")
    .and_then(Value::as_str)
    .filter(|value| !value.is_empty())
    .map(ToString::to_string)
}

fn requested_heavy(body: &Value) -> bool {
  body.get("heavy").and_then(Value::as_bool).unwrap_or(false)
}

fn dispatch(service: Arc<Service>, stop: Arc<AtomicBool>, path: &str, body: Value) -> Value {
  let async_job = body.get("async").and_then(Value::as_bool).unwrap_or(false);
  match path {
    "/v1/shutdown" => {
      stop.store(true, Ordering::Relaxed);
      json!({"ok": true, "pid": std::process::id()})
    }
    "/v1/job" => service
      .jobs
      .get(body.get("id").and_then(Value::as_str).unwrap_or(""))
      .map(|job| json!({"ok": true, "job": job}))
      .unwrap_or_else(|err| json!({"ok": false, "error": err.to_string()})),
    "/v1/sessions" => service
      .context
      .sessions_status()
      .unwrap_or_else(|err| json!({"ok": false, "error": err.to_string()})),
    "/v1/session/ensure" | "/v1/ensure" => {
      let session = requested_session(&body);
      if async_job {
        let jobs = service.jobs.clone();
        let service_for_job = Arc::clone(&service);
        jobs.submit("ensure", move || service_for_job.ensure_session(session))
      } else {
        service
          .ensure_session(session)
          .unwrap_or_else(|err| json!({"ok": false, "error": err.to_string()}))
      }
    }
    "/v1/session/mark-heavy" => {
      let session = body.get("session").and_then(Value::as_str).unwrap_or("");
      service
        .context
        .set_manual_heavy(session, true)
        .unwrap_or_else(|err| json!({"ok": false, "error": err.to_string()}))
    }
    "/v1/session/unmark-heavy" => {
      let session = body.get("session").and_then(Value::as_str).unwrap_or("");
      service
        .context
        .set_manual_heavy(session, false)
        .unwrap_or_else(|err| json!({"ok": false, "error": err.to_string()}))
    }
    "/v1/run" => {
      let command = body
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
      let timeout =
        Duration::from_secs_f64(body.get("timeout").and_then(Value::as_f64).unwrap_or(180.0));
      let (rows, cols) = terminal_rows_cols(
        body.get("rows").and_then(Value::as_u64),
        body.get("cols").and_then(Value::as_u64),
      );
      let raw = body.get("raw").and_then(Value::as_bool).unwrap_or(false);
      let no_prelude = body
        .get("no_prelude")
        .and_then(Value::as_bool)
        .unwrap_or(false);
      let ensure = body.get("ensure").and_then(Value::as_bool).unwrap_or(true);
      let session = requested_session(&body);
      let heavy = requested_heavy(&body);
      if async_job {
        let jobs = service.jobs.clone();
        let service_for_job = Arc::clone(&service);
        jobs.submit("run", move || {
          service_for_job.run_command(
            command, timeout, rows, cols, raw, no_prelude, ensure, session, heavy,
          )
        })
      } else {
        service
          .run_command(
            command, timeout, rows, cols, raw, no_prelude, ensure, session, heavy,
          )
          .unwrap_or_else(|err| json!({"ok": false, "error": err.to_string()}))
      }
    }
    "/v1/upload" => {
      let destination = body
        .get("destination")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
      let content_b64 = body
        .get("content_b64")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
      let mode = body.get("mode").and_then(Value::as_u64).unwrap_or(0o644) as u32;
      let is_archive = body
        .get("is_archive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
      let timeout =
        Duration::from_secs_f64(body.get("timeout").and_then(Value::as_f64).unwrap_or(180.0));
      let chunk_size = body
        .get("chunk_size")
        .and_then(Value::as_u64)
        .unwrap_or(262144) as usize;
      let session = requested_session(&body);
      let heavy = requested_heavy(&body);
      if async_job {
        let jobs = service.jobs.clone();
        let service_for_job = Arc::clone(&service);
        jobs.submit("upload", move || {
          service_for_job.upload(
            destination,
            content_b64,
            mode,
            is_archive,
            timeout,
            chunk_size,
            session,
            heavy,
          )
        })
      } else {
        service
          .upload(
            destination,
            content_b64,
            mode,
            is_archive,
            timeout,
            chunk_size,
            session,
            heavy,
          )
          .unwrap_or_else(|err| json!({"ok": false, "error": err.to_string()}))
      }
    }
    "/v1/download" => {
      let source = body
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
      let recursive = body
        .get("recursive")
        .and_then(Value::as_bool)
        .unwrap_or(false);
      let timeout =
        Duration::from_secs_f64(body.get("timeout").and_then(Value::as_f64).unwrap_or(180.0));
      let session = requested_session(&body);
      let heavy = requested_heavy(&body);
      if async_job {
        let jobs = service.jobs.clone();
        let service_for_job = Arc::clone(&service);
        jobs.submit("download", move || {
          service_for_job.download(source, recursive, timeout, session, heavy)
        })
      } else {
        service
          .download(source, recursive, timeout, session, heavy)
          .unwrap_or_else(|err| json!({"ok": false, "error": err.to_string()}))
      }
    }
    "/v1/terminal/start" => {
      let (rows, cols) = terminal_rows_cols(
        body.get("rows").and_then(Value::as_u64),
        body.get("cols").and_then(Value::as_u64),
      );
      service
        .start_terminal(rows, cols, requested_session(&body), requested_heavy(&body))
        .unwrap_or_else(|err| json!({"ok": false, "error": err.to_string()}))
    }
    "/v1/terminal/input" => {
      let id = body.get("id").and_then(Value::as_str).unwrap_or("");
      let data = body.get("data").and_then(Value::as_str).unwrap_or("");
      service
        .get_terminal(id)
        .and_then(|terminal| terminal.input(data))
        .map(|_| json!({"ok": true}))
        .unwrap_or_else(|err| json!({"ok": false, "error": err.to_string()}))
    }
    "/v1/terminal/read" => {
      let id = body.get("id").and_then(Value::as_str).unwrap_or("");
      let timeout =
        Duration::from_secs_f64(body.get("timeout").and_then(Value::as_f64).unwrap_or(1.0));
      service
        .get_terminal(id)
        .map(|terminal| json!({"ok": true, "output": terminal.read(timeout)}))
        .unwrap_or_else(|err| json!({"ok": false, "error": err.to_string()}))
    }
    "/v1/terminal/resize" => {
      let id = body.get("id").and_then(Value::as_str).unwrap_or("");
      let (rows, cols) = terminal_rows_cols(
        body.get("rows").and_then(Value::as_u64),
        body.get("cols").and_then(Value::as_u64),
      );
      service
        .get_terminal(id)
        .and_then(|terminal| terminal.resize(rows, cols))
        .map(|_| json!({"ok": true}))
        .unwrap_or_else(|err| json!({"ok": false, "error": err.to_string()}))
    }
    "/v1/terminal/close" => {
      let id = body.get("id").and_then(Value::as_str).unwrap_or("");
      service.close_terminal(id);
      json!({"ok": true})
    }
    _ => json!({"ok": false, "error": format!("unknown endpoint: {path}")}),
  }
}

pub(super) fn run_http_server(service: Arc<Service>, stop: Arc<AtomicBool>) -> Result<()> {
  let args = &service.context.args;
  let server = Server::http(format!("{}:{}", args.listen_host, args.listen_port))
    .map_err(|err| anyhow!("start HTTP server: {err}"))?;
  crate::util::log(format!(
    "gjtd API listening on http://{}:{}",
    args.listen_host, args.listen_port
  ));
  while !stop.load(Ordering::Relaxed) {
    let Some(mut request) = server.recv_timeout(Duration::from_millis(200))? else {
      continue;
    };
    let method = request.method().clone();
    let path = request.url().to_string();
    let service = Arc::clone(&service);
    let stop = Arc::clone(&stop);
    thread::spawn(move || {
      let response = match method {
        Method::Get if path == "/" || path == "/v1/health" => json!({
            "ok": true,
            "service": "gjtd",
            "pid": std::process::id(),
            "jobs": service.jobs.stats(),
            "stream_url": format!("tcp://{}:{}", service.context.args.stream_host, service.context.args.stream_port),
        }),
        Method::Get if path == "/v1/sessions" => service
          .context
          .sessions_status()
          .unwrap_or_else(|err| json!({"ok": false, "error": err.to_string()})),
        Method::Post => {
          let mut body_text = String::new();
          let read_result = request.as_reader().read_to_string(&mut body_text);
          match read_result {
            Ok(_) => {
              let body = if body_text.trim().is_empty() {
                json!({})
              } else {
                serde_json::from_str(&body_text)
                  .unwrap_or_else(|err| json!({"__parse_error": err.to_string()}))
              };
              if let Some(error) = body.get("__parse_error").and_then(Value::as_str) {
                json!({"ok": false, "error": error})
              } else {
                dispatch(service, stop, &path, body)
              }
            }
            Err(err) => json!({"ok": false, "error": err.to_string()}),
          }
        }
        _ => json!({"ok": false, "error": format!("unknown endpoint: {path}")}),
      };
      let data = serde_json::to_string(&response).unwrap_or_else(|_| "{\"ok\":false}".to_string());
      let status = if response.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        StatusCode(200)
      } else {
        StatusCode(200)
      };
      let _ = request.respond(
        Response::from_string(data)
          .with_status_code(status)
          .with_header(
            Header::from_bytes(
              b"Content-Type".as_slice(),
              b"application/json; charset=utf-8".as_slice(),
            )
            .unwrap(),
          ),
      );
    });
  }
  Ok(())
}
