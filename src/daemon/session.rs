use super::DaemonContext;
use crate::direct::{self, NotebookInfo};
use crate::util::token_hex;
use anyhow::{Result, anyhow, bail};
use regex::Regex;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug)]
pub(super) struct NotebookSpec {
  pub(super) session_id: String,
  pub(super) experience_index: usize,
  pub(super) repo_url: String,
  pub(super) ttl: String,
  pub(super) disk_size: String,
  pub(super) notebook_path: String,
  pub(super) scan_file_path: String,
  pub(super) gitcode_user: String,
}

pub(super) struct SessionLease {
  context: Arc<DaemonContext>,
  pub(super) session_id: String,
  heavy_owner: Option<String>,
}

impl SessionLease {
  pub(super) fn acquire(
    context: Arc<DaemonContext>,
    requested: Option<String>,
    heavy: bool,
    lease_duration: Duration,
  ) -> Result<Self> {
    let session_id = context.acquire_session_id(requested.as_deref(), heavy, lease_duration)?;
    let heavy_owner = if heavy {
      Some(context.mark_auto_heavy_locked_public(&session_id, lease_duration)?)
    } else {
      None
    };
    Ok(Self {
      context,
      session_id,
      heavy_owner,
    })
  }
}

impl Drop for SessionLease {
  fn drop(&mut self) {
    if let Some(owner) = self.heavy_owner.as_deref() {
      let _ = self.context.release_auto_heavy(&self.session_id, owner);
    }
  }
}

#[derive(Clone, Debug)]
struct HubExperience {
  repo_url: String,
  ttl: String,
  disk_size: String,
  notebook_path: String,
  scan_file_path: String,
}

impl DaemonContext {
  pub(super) fn configured_session_ids(&self) -> Vec<String> {
    let mut ids = Vec::new();
    let text = self.args.session_experiences.trim();
    if !text.is_empty() {
      for item in text
        .split(|ch: char| ch == ',' || ch == ';' || ch.is_whitespace())
        .map(str::trim)
        .filter(|item| !item.is_empty())
      {
        if !ids.iter().any(|id| id == item) {
          ids.push(item.to_string());
        }
      }
    }
    if ids.is_empty() {
      ids.push(self.args.experience_index.to_string());
    }
    ids
  }

  pub(super) fn session_spec(&self, session_id: &str) -> Result<NotebookSpec> {
    let experience_index = parse_session_id(session_id)?;
    let mut spec = self.default_session_spec(session_id, experience_index);
    match self.resolve_hub_experience(experience_index) {
      Ok(Some(experience)) => {
        spec.repo_url = experience.repo_url;
        spec.ttl = experience.ttl;
        spec.disk_size = experience.disk_size;
        spec.notebook_path = experience.notebook_path;
        spec.scan_file_path = experience.scan_file_path;
      }
      Ok(None) if experience_index == self.args.experience_index => {}
      Ok(None) => {
        if let Some(stored) = self.session_spec_from_state(session_id, experience_index) {
          return Ok(stored);
        }
        bail!(
          "hub experience index {experience_index} is unavailable; configure --session-experiences with valid indexes"
        );
      }
      Err(err) if experience_index == self.args.experience_index => {
        crate::util::log(format!(
          "failed to fetch hub experiences; using configured defaults for session {session_id}: {err}"
        ));
      }
      Err(err) => {
        if let Some(stored) = self.session_spec_from_state(session_id, experience_index) {
          return Ok(stored);
        }
        bail!("failed to resolve hub experience index {experience_index}: {err}");
      }
    }
    Ok(spec)
  }

  fn default_session_spec(&self, session_id: &str, experience_index: usize) -> NotebookSpec {
    NotebookSpec {
      session_id: session_id.to_string(),
      experience_index,
      repo_url: self.args.repo_url.clone(),
      ttl: self.args.ttl.clone(),
      disk_size: self.args.disk_size.clone(),
      notebook_path: self.args.notebook_path.clone(),
      scan_file_path: self.args.scan_file_path.clone(),
      gitcode_user: self.args.gitcode_user.clone(),
    }
  }

  fn session_spec_from_state(
    &self,
    session_id: &str,
    experience_index: usize,
  ) -> Option<NotebookSpec> {
    let _guard = self.state_lock.lock().unwrap();
    let state = self.normalized_state_locked().ok()?;
    let session = state.get("sessions")?.get(session_id)?;
    Some(NotebookSpec {
      session_id: session_id.to_string(),
      experience_index,
      repo_url: session.get("repo_url")?.as_str()?.to_string(),
      ttl: session
        .get("ttl")
        .and_then(Value::as_str)
        .unwrap_or(&self.args.ttl)
        .to_string(),
      disk_size: session
        .get("disk_size")
        .and_then(Value::as_str)
        .unwrap_or(&self.args.disk_size)
        .to_string(),
      notebook_path: session.get("notebook_path")?.as_str()?.to_string(),
      scan_file_path: session.get("scan_file_path")?.as_str()?.to_string(),
      gitcode_user: self.args.gitcode_user.clone(),
    })
  }

  pub(super) fn previous_notebook_from_session_state(
    &self,
    session_id: &str,
  ) -> Option<NotebookInfo> {
    let _guard = self.state_lock.lock().unwrap();
    let state = self.normalized_state_locked().ok()?;
    let session = state.get("sessions")?.get(session_id)?;
    let href = session.get("href")?.as_str()?.to_string();
    if !href.contains("aihub-run.gitcode.com") {
      return None;
    }
    direct::notebook_from_lab_url(
      &href,
      session
        .get("target_url")
        .and_then(Value::as_str)
        .unwrap_or(""),
      json!({
          "state_file": self.state_file_path().display().to_string(),
          "state_time": session.get("time").cloned().unwrap_or(Value::Null),
          "session": session_id,
          "experience_index": session.get("experience_index").cloned().unwrap_or(Value::Null),
          "status": session.get("status").cloned().unwrap_or_else(|| json!({})),
          "notebook_id": session.get("notebook_id").cloned().unwrap_or(Value::Null),
      }),
    )
    .ok()
  }

  pub(super) fn record_session_ok_state(&self, spec: &NotebookSpec, probe: &Value) -> Result<()> {
    let _guard = self.state_lock.lock().unwrap();
    let mut state = self.normalized_state_locked()?;
    let sessions = ensure_sessions_object(&mut state);
    let old = sessions
      .get(&spec.session_id)
      .and_then(Value::as_object)
      .cloned()
      .unwrap_or_default();
    let status = probe.get("status").cloned().unwrap_or_else(|| json!({}));
    let mut entry = Map::new();
    entry.insert("id".to_string(), json!(spec.session_id));
    entry.insert("experience_index".to_string(), json!(spec.experience_index));
    entry.insert("repo_url".to_string(), json!(spec.repo_url));
    entry.insert("ttl".to_string(), json!(spec.ttl));
    entry.insert("disk_size".to_string(), json!(spec.disk_size));
    entry.insert("notebook_path".to_string(), json!(spec.notebook_path));
    entry.insert("scan_file_path".to_string(), json!(spec.scan_file_path));
    entry.insert("ok".to_string(), json!(true));
    entry.insert(
      "href".to_string(),
      probe.get("href").cloned().unwrap_or(Value::Null),
    );
    entry.insert(
      "target_url".to_string(),
      probe.get("target_url").cloned().unwrap_or(Value::Null),
    );
    entry.insert(
      "notebook_id".to_string(),
      probe.get("notebook_id").cloned().unwrap_or(Value::Null),
    );
    entry.insert(
      "base_url".to_string(),
      probe.get("base_url").cloned().unwrap_or(Value::Null),
    );
    entry.insert("status".to_string(), status.clone());
    entry.insert(
      "remote_started".to_string(),
      status.get("started").cloned().unwrap_or(Value::Null),
    );
    entry.insert(
      "remote_last_activity".to_string(),
      status.get("last_activity").cloned().unwrap_or(Value::Null),
    );
    entry.insert(
      "target_url_contains".to_string(),
      json!(self.args.notebook_target_contains),
    );
    entry.insert(
      "page_url_contains".to_string(),
      json!(self.args.notebook_page_contains),
    );
    entry.insert(
      "profile".to_string(),
      json!(
        crate::config::expand_tilde(&self.args.chrome_user_data_dir)
          .display()
          .to_string()
      ),
    );
    entry.insert(
      "probe".to_string(),
      probe.get("output").cloned().unwrap_or(Value::Null),
    );
    preserve_heavy_fields(&old, &mut entry);
    entry.insert("last_used".to_string(), json!(direct::now()));
    entry.insert("time".to_string(), json!(direct::now()));
    sessions.insert(spec.session_id.clone(), Value::Object(entry));
    update_top_level_ok(&mut state);
    self.write_state_value_locked(&state)
  }

  pub(super) fn record_session_failure(&self, session_id: &str, reason: &str) -> Result<()> {
    let _guard = self.state_lock.lock().unwrap();
    let mut state = self.normalized_state_locked()?;
    let sessions = ensure_sessions_object(&mut state);
    let mut entry = sessions
      .get(session_id)
      .and_then(Value::as_object)
      .cloned()
      .unwrap_or_default();
    entry.insert("id".to_string(), json!(session_id));
    if let Ok(index) = parse_session_id(session_id) {
      entry.insert("experience_index".to_string(), json!(index));
    }
    entry.insert("ok".to_string(), json!(false));
    entry.insert("reason".to_string(), json!(reason));
    entry.insert("time".to_string(), json!(direct::now()));
    sessions.insert(session_id.to_string(), Value::Object(entry));
    update_top_level_ok(&mut state);
    self.write_state_value_locked(&state)
  }

  pub(super) fn sessions_status(&self) -> Result<Value> {
    let _guard = self.state_lock.lock().unwrap();
    let mut state = self.normalized_state_locked()?;
    clear_expired_heavy_locked(&mut state);
    let configured = self.configured_session_ids();
    let sessions = ensure_sessions_object(&mut state);
    for id in &configured {
      sessions.entry(id.clone()).or_insert_with(|| {
        json!({
          "id": id,
          "experience_index": parse_session_id(id).ok(),
          "ok": false,
          "configured": true,
          "heavy": false,
        })
      });
      if let Some(entry) = sessions.get_mut(id).and_then(Value::as_object_mut) {
        entry.insert("configured".to_string(), json!(true));
      }
    }
    let mut list = sessions.values().cloned().collect::<Vec<_>>();
    list.sort_by(|a, b| {
      let a_id = a.get("id").and_then(Value::as_str).unwrap_or("");
      let b_id = b.get("id").and_then(Value::as_str).unwrap_or("");
      session_sort_key(a_id).cmp(&session_sort_key(b_id))
    });
    self.write_state_value_locked(&state)?;
    Ok(json!({"ok": true, "configured_sessions": configured, "sessions": list}))
  }

  pub(super) fn set_manual_heavy(&self, session_id: &str, heavy: bool) -> Result<Value> {
    parse_session_id(session_id)?;
    let _guard = self.state_lock.lock().unwrap();
    let mut state = self.normalized_state_locked()?;
    clear_expired_heavy_locked(&mut state);
    let sessions = ensure_sessions_object(&mut state);
    let mut entry = sessions
      .get(session_id)
      .and_then(Value::as_object)
      .cloned()
      .unwrap_or_default();
    entry.insert("id".to_string(), json!(session_id));
    entry.insert(
      "experience_index".to_string(),
      json!(parse_session_id(session_id)?),
    );
    entry.insert("heavy".to_string(), json!(heavy));
    if heavy {
      entry.insert("heavy_owner".to_string(), json!("manual"));
      entry.insert("heavy_since".to_string(), json!(direct::now()));
      entry.insert("heavy_until".to_string(), Value::Null);
    } else {
      entry.insert("heavy_owner".to_string(), Value::Null);
      entry.insert("heavy_since".to_string(), Value::Null);
      entry.insert("heavy_until".to_string(), Value::Null);
    }
    entry.insert("time".to_string(), json!(direct::now()));
    sessions.insert(session_id.to_string(), Value::Object(entry.clone()));
    update_top_level_ok(&mut state);
    self.write_state_value_locked(&state)?;
    Ok(json!({"ok": true, "session": Value::Object(entry)}))
  }

  fn acquire_session_id(
    &self,
    requested: Option<&str>,
    heavy: bool,
    lease_duration: Duration,
  ) -> Result<String> {
    let _guard = self.state_lock.lock().unwrap();
    let mut state = self.normalized_state_locked()?;
    clear_expired_heavy_locked(&mut state);
    let sessions = ensure_sessions_object(&mut state);
    let mut candidates = if let Some(id) = requested.filter(|id| !id.trim().is_empty()) {
      parse_session_id(id)?;
      vec![id.to_string()]
    } else {
      let mut ids = self.configured_session_ids();
      for id in sessions.keys() {
        if !ids.iter().any(|known| known == id) && parse_session_id(id).is_ok() {
          ids.push(id.clone());
        }
      }
      ids
    };
    candidates.retain(|id| parse_session_id(id).is_ok());
    if candidates.is_empty() {
      bail!("no configured sessions are available");
    }
    let now = direct::now();
    let mut available = candidates
      .into_iter()
      .filter(|id| !heavy || !session_is_heavy(sessions.get(id), now))
      .collect::<Vec<_>>();
    if available.is_empty() {
      if let Some(id) = requested {
        bail!("session {id} is already marked heavy");
      }
      bail!("no non-heavy session is available for a heavy command");
    }
    available.sort_by(|a, b| {
      let a_last = session_last_used(sessions.get(a));
      let b_last = session_last_used(sessions.get(b));
      a_last
        .partial_cmp(&b_last)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| session_sort_key(a).cmp(&session_sort_key(b)))
    });
    let selected = available.remove(0);
    let mut entry = sessions
      .get(&selected)
      .and_then(Value::as_object)
      .cloned()
      .unwrap_or_default();
    entry.insert("id".to_string(), json!(selected));
    entry.insert(
      "experience_index".to_string(),
      json!(parse_session_id(&selected)?),
    );
    entry.insert("last_used".to_string(), json!(now));
    entry.entry("heavy".to_string()).or_insert(json!(false));
    if heavy {
      let owner = format!("pending-{}", token_hex(8));
      entry.insert("heavy".to_string(), json!(true));
      entry.insert("heavy_owner".to_string(), json!(owner));
      entry.insert("heavy_since".to_string(), json!(now));
      entry.insert(
        "heavy_until".to_string(),
        json!(now + lease_duration.as_secs_f64().max(1.0)),
      );
    }
    sessions.insert(selected.clone(), Value::Object(entry));
    update_top_level_ok(&mut state);
    self.write_state_value_locked(&state)?;
    Ok(selected)
  }

  fn mark_auto_heavy_locked_public(
    &self,
    session_id: &str,
    lease_duration: Duration,
  ) -> Result<String> {
    let owner = format!("auto-{}", token_hex(8));
    let _guard = self.state_lock.lock().unwrap();
    let mut state = self.normalized_state_locked()?;
    clear_expired_heavy_locked(&mut state);
    let sessions = ensure_sessions_object(&mut state);
    let now = direct::now();
    let mut entry = sessions
      .get(session_id)
      .and_then(Value::as_object)
      .cloned()
      .unwrap_or_default();
    if session_is_heavy(Some(&Value::Object(entry.clone())), now)
      && entry.get("heavy_owner").and_then(Value::as_str) != Some("manual")
    {
      // acquire_session_id already reserved this session with a pending owner. Replace it.
    } else if session_is_heavy(Some(&Value::Object(entry.clone())), now) {
      bail!("session {session_id} is already marked heavy");
    }
    entry.insert("id".to_string(), json!(session_id));
    entry.insert(
      "experience_index".to_string(),
      json!(parse_session_id(session_id)?),
    );
    entry.insert("heavy".to_string(), json!(true));
    entry.insert("heavy_owner".to_string(), json!(owner.clone()));
    entry.insert("heavy_since".to_string(), json!(now));
    entry.insert(
      "heavy_until".to_string(),
      json!(now + lease_duration.as_secs_f64().max(1.0)),
    );
    entry.insert("last_used".to_string(), json!(now));
    sessions.insert(session_id.to_string(), Value::Object(entry));
    update_top_level_ok(&mut state);
    self.write_state_value_locked(&state)?;
    Ok(owner)
  }

  fn release_auto_heavy(&self, session_id: &str, owner: &str) -> Result<()> {
    let _guard = self.state_lock.lock().unwrap();
    let mut state = self.normalized_state_locked()?;
    let sessions = ensure_sessions_object(&mut state);
    if let Some(entry) = sessions.get_mut(session_id).and_then(Value::as_object_mut)
      && entry.get("heavy_owner").and_then(Value::as_str) == Some(owner)
    {
      entry.insert("heavy".to_string(), json!(false));
      entry.insert("heavy_owner".to_string(), Value::Null);
      entry.insert("heavy_since".to_string(), Value::Null);
      entry.insert("heavy_until".to_string(), Value::Null);
      entry.insert("time".to_string(), json!(direct::now()));
    }
    update_top_level_ok(&mut state);
    self.write_state_value_locked(&state)
  }

  fn normalized_state_locked(&self) -> Result<Value> {
    let path = self.state_file_path();
    let value = match std::fs::read_to_string(&path) {
      Ok(text) => serde_json::from_str::<Value>(&text).unwrap_or_else(|_| json!({})),
      Err(_) => json!({}),
    };
    Ok(self.normalize_state(value))
  }

  fn normalize_state(&self, value: Value) -> Value {
    if value.get("version").and_then(Value::as_u64) == Some(2)
      && value.get("sessions").and_then(Value::as_object).is_some()
    {
      return value;
    }
    let mut sessions = Map::new();
    if value.get("href").and_then(Value::as_str).is_some() {
      let id = self.args.experience_index.to_string();
      let mut entry = value.as_object().cloned().unwrap_or_default();
      entry.insert("id".to_string(), json!(id));
      entry.insert(
        "experience_index".to_string(),
        json!(self.args.experience_index),
      );
      entry.entry("heavy".to_string()).or_insert(json!(false));
      sessions.insert(id, Value::Object(entry));
    }
    json!({
      "version": 2,
      "app": crate::config::APP_NAME,
      "sessions": sessions,
      "time": direct::now(),
    })
  }

  fn write_state_value_locked(&self, state: &Value) -> Result<()> {
    if self.args.state_file.is_empty() {
      return Ok(());
    }
    crate::util::write_json_file(&self.state_file_path(), state)
  }

  fn resolve_hub_experience(&self, index: usize) -> Result<Option<HubExperience>> {
    let experiences = self.fetch_hub_experiences()?;
    Ok(experiences.get(index).cloned())
  }

  fn fetch_hub_experiences(&self) -> Result<Vec<HubExperience>> {
    let client = reqwest::blocking::Client::builder().no_proxy().build()?;
    let body = client
      .get(&self.args.hub_url)
      .timeout(Duration::from_secs_f64(self.args.probe_timeout.max(5.0)))
      .send()
      .and_then(|response| response.error_for_status())
      .and_then(|response| response.text())
      .map_err(|err| anyhow!("fetch hub experiences from {}: {err}", self.args.hub_url))?;
    Ok(parse_hub_experiences(&body))
  }
}

fn parse_session_id(session_id: &str) -> Result<usize> {
  session_id
    .parse::<usize>()
    .map_err(|_| anyhow!("session id must be a hub experience index, got {session_id:?}"))
}

fn parse_hub_experiences(body: &str) -> Vec<HubExperience> {
  let Ok(re) =
    Regex::new(r#"https://ai\.gitcode\.com/user/[^\"'\s<>)]+/notebookcann\?[^\"'\s<>)]+"#)
  else {
    return Vec::new();
  };
  let mut seen = HashSet::new();
  let mut out = Vec::new();
  for mat in re.find_iter(body) {
    let raw = mat.as_str().replace("&amp;", "&");
    let Ok(url) = url::Url::parse(&raw) else {
      continue;
    };
    let mut repo_url = String::new();
    let mut ttl = String::new();
    let mut disk_size = String::new();
    let mut notebook_path = String::new();
    let mut scan_file_path = String::new();
    for (key, value) in url.query_pairs() {
      match key.as_ref() {
        "repoUrl" => repo_url = value.into_owned(),
        "ttl" => ttl = value.into_owned(),
        "diskSize" => disk_size = value.into_owned(),
        "path" => notebook_path = value.into_owned(),
        "scanFilePath" => scan_file_path = value.into_owned(),
        _ => {}
      }
    }
    if repo_url.is_empty() || notebook_path.is_empty() || scan_file_path.is_empty() {
      continue;
    }
    let key = format!("{repo_url}\n{notebook_path}\n{scan_file_path}");
    if !seen.insert(key) {
      continue;
    }
    out.push(HubExperience {
      repo_url,
      ttl: if ttl.is_empty() {
        "120".to_string()
      } else {
        ttl
      },
      disk_size: if disk_size.is_empty() {
        "40Gi".to_string()
      } else {
        disk_size
      },
      notebook_path,
      scan_file_path,
    });
  }
  out
}

fn ensure_sessions_object(state: &mut Value) -> &mut Map<String, Value> {
  if !state.is_object() {
    *state = json!({});
  }
  let object = state.as_object_mut().unwrap();
  object.insert("version".to_string(), json!(2));
  object
    .entry("app".to_string())
    .or_insert(json!(crate::config::APP_NAME));
  object
    .entry("time".to_string())
    .or_insert_with(|| json!(direct::now()));
  if !object.get("sessions").and_then(Value::as_object).is_some() {
    object.insert("sessions".to_string(), json!({}));
  }
  object
    .get_mut("sessions")
    .and_then(Value::as_object_mut)
    .unwrap()
}

fn update_top_level_ok(state: &mut Value) {
  let any_ok = state
    .get("sessions")
    .and_then(Value::as_object)
    .is_some_and(|sessions| {
      sessions
        .values()
        .any(|session| session.get("ok").and_then(Value::as_bool).unwrap_or(false))
    });
  if let Some(object) = state.as_object_mut() {
    object.insert("ok".to_string(), json!(any_ok));
    object.insert("time".to_string(), json!(direct::now()));
  }
}

fn preserve_heavy_fields(old: &Map<String, Value>, entry: &mut Map<String, Value>) {
  for key in ["heavy", "heavy_owner", "heavy_since", "heavy_until"] {
    entry.insert(
      key.to_string(),
      old.get(key).cloned().unwrap_or_else(|| {
        if key == "heavy" {
          json!(false)
        } else {
          Value::Null
        }
      }),
    );
  }
}

fn clear_expired_heavy_locked(state: &mut Value) {
  let now = direct::now();
  if let Some(sessions) = state.get_mut("sessions").and_then(Value::as_object_mut) {
    for session in sessions.values_mut() {
      if session_is_heavy(Some(session), now) {
        continue;
      }
      if let Some(entry) = session.as_object_mut()
        && entry.get("heavy").and_then(Value::as_bool).unwrap_or(false)
      {
        entry.insert("heavy".to_string(), json!(false));
        entry.insert("heavy_owner".to_string(), Value::Null);
        entry.insert("heavy_since".to_string(), Value::Null);
        entry.insert("heavy_until".to_string(), Value::Null);
      }
    }
  }
}

fn session_is_heavy(session: Option<&Value>, now: f64) -> bool {
  let Some(session) = session.and_then(Value::as_object) else {
    return false;
  };
  if !session
    .get("heavy")
    .and_then(Value::as_bool)
    .unwrap_or(false)
  {
    return false;
  }
  match session.get("heavy_until") {
    Some(Value::Number(number)) => number.as_f64().is_none_or(|until| until > now),
    Some(Value::Null) | None => true,
    _ => true,
  }
}

fn session_last_used(session: Option<&Value>) -> f64 {
  session
    .and_then(|session| session.get("last_used"))
    .and_then(Value::as_f64)
    .unwrap_or(0.0)
}

fn session_sort_key(id: &str) -> (usize, String) {
  (id.parse::<usize>().unwrap_or(usize::MAX), id.to_string())
}
