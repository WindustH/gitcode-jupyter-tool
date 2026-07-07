use crate::direct;
use crate::util::token_hex;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone)]
pub(crate) struct JobManager {
  retention: Duration,
  jobs: Arc<Mutex<HashMap<String, Value>>>,
}

impl JobManager {
  pub(crate) fn new(retention: Duration) -> Self {
    Self {
      retention,
      jobs: Arc::new(Mutex::new(HashMap::new())),
    }
  }

  pub(crate) fn submit<F>(&self, name: &str, func: F) -> Value
  where
    F: FnOnce() -> Result<Value> + Send + 'static,
  {
    let id = token_hex(12);
    let submitted = direct::now();
    {
      let mut jobs = self.jobs.lock().unwrap();
      self.prune_locked(&mut jobs);
      jobs.insert(
        id.clone(),
        json!({"id": id, "name": name, "status": "queued", "submitted": submitted}),
      );
    }
    let jobs = Arc::clone(&self.jobs);
    let job_id = id.clone();
    thread::spawn(move || {
      {
        let mut jobs = jobs.lock().unwrap();
        if let Some(job) = jobs.get_mut(&job_id) {
          *job = merge_job(job, json!({"status": "running", "started": direct::now()}));
        }
      }
      let update = match func() {
        Ok(result) => {
          json!({"status": "succeeded", "ok": true, "result": result, "finished": direct::now()})
        }
        Err(err) => {
          json!({"status": "failed", "ok": false, "error": err.to_string(), "finished": direct::now()})
        }
      };
      let mut jobs = jobs.lock().unwrap();
      if let Some(job) = jobs.get_mut(&job_id) {
        *job = merge_job(job, update);
      }
    });
    json!({"ok": true, "job_id": id, "job": self.get(&id).unwrap_or_else(|_| json!({}))})
  }

  pub(crate) fn get(&self, id: &str) -> Result<Value> {
    let jobs = self.jobs.lock().unwrap();
    jobs
      .get(id)
      .cloned()
      .ok_or_else(|| anyhow!("unknown job id: {id}"))
  }

  pub(crate) fn stats(&self) -> Value {
    let jobs = self.jobs.lock().unwrap();
    let mut counts: HashMap<String, usize> = HashMap::new();
    for job in jobs.values() {
      let status = job
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
      *counts.entry(status.to_string()).or_insert(0) += 1;
    }
    json!(counts)
  }

  fn prune_locked(&self, jobs: &mut HashMap<String, Value>) {
    let now = direct::now();
    let retention = self.retention.as_secs_f64();
    jobs.retain(|_, job| {
      if matches!(
        job.get("status").and_then(Value::as_str),
        Some("queued" | "running")
      ) {
        return true;
      }
      let finished = job
        .get("finished")
        .and_then(Value::as_f64)
        .unwrap_or_else(direct::now);
      now - finished <= retention
    });
  }
}

fn merge_job(base: &Value, update: Value) -> Value {
  let mut object = base.as_object().cloned().unwrap_or_default();
  if let Some(update) = update.as_object() {
    for (key, value) in update {
      object.insert(key.clone(), value.clone());
    }
  }
  Value::Object(object)
}
