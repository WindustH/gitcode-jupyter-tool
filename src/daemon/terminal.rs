use super::Service;
use crate::direct::ApiTerminal;
use crate::util::{log, shell_quote, strip_terminal_noise, token_hex};
use anyhow::{Result, anyhow};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const INTERACTIVE_PS1: &str = r"\[\033[1;36m\]\w\[\033[0m\] \[\033[1;32m\]\$\[\033[0m\] ";

pub(super) struct LiveTerminal {
  pub(super) id: String,
  pub(super) href: String,
  terminal: Arc<Mutex<ApiTerminal>>,
  output: Arc<(Mutex<VecDeque<String>>, Condvar)>,
  closed: Arc<AtomicBool>,
}

impl LiveTerminal {
  pub(super) fn new(service: &Service, rows: u16, cols: u16) -> Result<Self> {
    let mut last_error: Option<anyhow::Error> = None;
    for attempt in 0..2 {
      let context = service.direct_context(attempt > 0)?;
      let mut terminal = ApiTerminal::new(context.client, context.notebook.clone(), rows, cols);
      match terminal.start(Duration::from_secs(30)) {
        Ok(_) => {
          let id = token_hex(12);
          let href = context.notebook.lab_url;
          let terminal = Arc::new(Mutex::new(terminal));
          let output = Arc::new((Mutex::new(VecDeque::new()), Condvar::new()));
          let closed = Arc::new(AtomicBool::new(false));
          Self::spawn_pump(
            Arc::clone(&terminal),
            Arc::clone(&output),
            Arc::clone(&closed),
          );
          return Ok(Self {
            id,
            href,
            terminal,
            output,
            closed,
          });
        }
        Err(err) => {
          last_error = Some(err);
          if attempt == 0 {
            log("interactive terminal failed on current notebook; replacing notebook");
          }
        }
      }
    }
    Err(last_error.unwrap_or_else(|| anyhow!("interactive terminal failed")))
  }

  fn spawn_pump(
    terminal: Arc<Mutex<ApiTerminal>>,
    output: Arc<(Mutex<VecDeque<String>>, Condvar)>,
    closed: Arc<AtomicBool>,
  ) {
    thread::spawn(move || {
      while !closed.load(Ordering::Relaxed) {
        let text = {
          let mut terminal = terminal.lock().unwrap();
          terminal.read_once(Duration::from_millis(20))
        };
        match text {
          Ok(text) if !text.is_empty() => {
            let (lock, condvar) = &*output;
            let mut queue = lock.lock().unwrap();
            queue.push_back(text);
            condvar.notify_all();
          }
          Ok(_) => {}
          Err(err) => {
            let (lock, condvar) = &*output;
            let mut queue = lock.lock().unwrap();
            queue.push_back(format!("\n[gjtd terminal pump failed: {err}]\n"));
            condvar.notify_all();
            closed.store(true, Ordering::Relaxed);
          }
        }
      }
    });
  }

  pub(super) fn input(&self, data: &str) -> Result<()> {
    self.terminal.lock().unwrap().send(data)
  }

  pub(super) fn resize(&self, rows: u16, cols: u16) -> Result<()> {
    self.terminal.lock().unwrap().set_size(rows, cols)
  }

  pub(super) fn read(&self, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    let (lock, condvar) = &*self.output;
    let mut queue = lock.lock().unwrap();
    while queue.is_empty() && !self.closed.load(Ordering::Relaxed) && Instant::now() < deadline {
      let remaining = deadline.saturating_duration_since(Instant::now());
      let (next, _) = condvar.wait_timeout(queue, remaining).unwrap();
      queue = next;
    }
    let mut chunks = Vec::new();
    while let Some(chunk) = queue.pop_front() {
      chunks.push(chunk);
    }
    chunks.join("")
  }

  pub(super) fn prepare_interactive_prompt(&self) -> Result<String> {
    let _ = self.read(Duration::from_millis(200));
    self.input("stty -echo 2>/dev/null || true\n")?;
    let _ = self.read(Duration::from_millis(200));
    let marker = format!("__GJTD_PROMPT_READY_{}__", token_hex(8));
    let prelude = [
      "bind 'set enable-bracketed-paste off' 2>/dev/null || true".to_string(),
      "alias ll='ls -l --color=auto'".to_string(),
      "export PROMPT_COMMAND=".to_string(),
      format!("export PS1={}", shell_quote(INTERACTIVE_PS1)),
      format!("printf '\\033[2K\\r%s\\n' {}", shell_quote(&marker)),
      String::new(),
    ]
    .join("\n");
    self.input(&prelude)?;
    let mut collected = String::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline && !collected.contains(&marker) {
      collected.push_str(&self.read(Duration::from_millis(50)));
    }
    let settle = Instant::now() + Duration::from_millis(250);
    while Instant::now() < settle {
      collected.push_str(&self.read(Duration::from_millis(50)));
    }
    if !collected.contains(&marker) {
      return Ok(String::new());
    }
    self.input("stty echo 2>/dev/null || true\n")?;
    Ok(
      self
        .read(Duration::from_millis(500))
        .trim_start_matches(['\r', '\n'])
        .to_string(),
    )
  }

  pub(super) fn close(&self) {
    self.closed.store(true, Ordering::Relaxed);
    self.output.1.notify_all();
    self.terminal.lock().unwrap().close();
  }
}

pub(in crate::daemon) fn terminal_rows_cols(rows: Option<u64>, cols: Option<u64>) -> (u16, u16) {
  (
    rows.unwrap_or(24).max(1) as u16,
    cols.unwrap_or(120).max(1) as u16,
  )
}

pub(in crate::daemon) fn drain_api_terminal(
  terminal: &mut ApiTerminal,
  duration: Duration,
) -> Result<String> {
  let deadline = Instant::now() + duration;
  let mut collected = String::new();
  while Instant::now() < deadline {
    let chunk = terminal.read_once(
      Duration::from_millis(100).min(deadline.saturating_duration_since(Instant::now())),
    )?;
    collected.push_str(&chunk);
  }
  Ok(collected)
}

pub(in crate::daemon) fn wait_api_command_marker(
  terminal: &mut ApiTerminal,
  marker: &str,
  timeout: Duration,
  raw: bool,
) -> Result<(i32, String)> {
  let mut collected = String::new();
  let deadline = Instant::now() + timeout;
  while Instant::now() < deadline {
    let chunk = terminal.read_once(Duration::from_millis(100))?;
    if !chunk.is_empty() {
      collected.push_str(&chunk);
    }
    if let Some((status, text)) = find_marker(&collected, marker) {
      return Ok((
        status,
        if raw {
          text
        } else {
          strip_terminal_noise(&text)
        },
      ));
    }
    if collected.len() > 16 * 1024 * 1024 {
      let keep = collected.split_off(collected.len() - 8 * 1024 * 1024);
      collected = keep;
    }
  }
  Ok((
    124,
    if raw {
      collected
    } else {
      strip_terminal_noise(&collected)
    },
  ))
}

fn find_marker(collected: &str, marker: &str) -> Option<(i32, String)> {
  let index = collected.find(marker)?;
  let rest = &collected[index + marker.len()..];
  let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
  if digits.is_empty() {
    return None;
  }
  Some((digits.parse().ok()?, collected[..index].to_string()))
}

pub(in crate::daemon) fn read_stream_terminal_for(
  terminal: &mut ApiTerminal,
  duration: Duration,
) -> Result<String> {
  let deadline = Instant::now() + duration;
  let mut collected = String::new();
  while Instant::now() < deadline {
    let timeout = Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now()));
    let chunk = terminal.read_once(timeout)?;
    if !chunk.is_empty() {
      collected.push_str(&chunk);
    }
  }
  Ok(collected)
}

pub(in crate::daemon) fn prepare_stream_prompt(terminal: &mut ApiTerminal) -> Result<String> {
  let _ = read_stream_terminal_for(terminal, Duration::from_millis(200));
  terminal.send("stty -echo 2>/dev/null || true\n")?;
  let _ = read_stream_terminal_for(terminal, Duration::from_millis(200));

  let marker = format!("__GJTD_PROMPT_READY_{}__", token_hex(8));
  let prelude = [
    "bind 'set enable-bracketed-paste off' 2>/dev/null || true".to_string(),
    "alias ll='ls -l --color=auto'".to_string(),
    "export PROMPT_COMMAND=".to_string(),
    format!("export PS1={}", shell_quote(INTERACTIVE_PS1)),
    format!("printf '\\033[2K\\r%s\\n' {}", shell_quote(&marker)),
    String::new(),
  ]
  .join("\n");
  terminal.send(&prelude)?;

  let mut collected = String::new();
  let deadline = Instant::now() + Duration::from_secs(3);
  while Instant::now() < deadline && !collected.contains(&marker) {
    collected.push_str(&terminal.read_once(Duration::from_millis(50))?);
  }
  let settle = Instant::now() + Duration::from_millis(250);
  while Instant::now() < settle {
    collected.push_str(&terminal.read_once(Duration::from_millis(50))?);
  }

  if !collected.contains(&marker) {
    return Ok(String::new());
  }

  terminal.send("stty echo 2>/dev/null || true\n")?;
  Ok(
    read_stream_terminal_for(terminal, Duration::from_millis(500))?
      .trim_start_matches(['\r', '\n'])
      .to_string(),
  )
}
