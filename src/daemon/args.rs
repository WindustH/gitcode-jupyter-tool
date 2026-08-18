use crate::config;
use clap::{ArgAction, Parser};

fn default_hub_url() -> String {
  config::DEFAULT_HUB_URL.to_string()
}

fn default_repo_url() -> String {
  config::env_string(&["JUD_REPO_URL"], config::DEFAULT_REPO_URL)
}

fn default_ttl() -> String {
  config::env_string(&["JUD_TTL"], "120")
}

fn default_disk_size() -> String {
  config::env_string(&["JUD_DISK_SIZE"], "40Gi")
}

fn default_notebook_path() -> String {
  config::env_string(&["JUD_NOTEBOOK_PATH"], config::DEFAULT_NOTEBOOK_PATH)
}

fn default_scan_file_path() -> String {
  config::env_string(&["JUD_SCAN_FILE_PATH"], config::DEFAULT_SCAN_FILE_PATH)
}

fn default_gitcode_user() -> String {
  config::env_string(&["GITCODE_USER"], "username")
}

fn default_profile_directory() -> String {
  config::env_string(&["JUD_CHROME_PROFILE"], "Default")
}

#[derive(Clone, Parser)]
#[command(
  name = "jud",
  about = "Maintain an available GitCode CANN JupyterLab notebook instance."
)]
pub(crate) struct Args {
  #[arg(long, default_value_t = config::default_account())]
  pub(crate) account: String,
  #[arg(long, default_value_t = default_hub_url())]
  pub(crate) hub_url: String,
  #[arg(long, default_value = "gitcode.com/cann/cann-learning-hub")]
  pub(crate) hub_target_contains: String,
  #[arg(long, default_value = "gitcode.com/cann/cann-learning-hub")]
  pub(crate) hub_context_contains: String,
  #[arg(long, default_value_t = 0)]
  pub(crate) experience_index: usize,
  #[arg(long, default_value_t = default_repo_url())]
  pub(crate) repo_url: String,
  #[arg(long, default_value_t = default_ttl())]
  pub(crate) ttl: String,
  #[arg(long, default_value_t = default_disk_size())]
  pub(crate) disk_size: String,
  #[arg(long, default_value_t = default_notebook_path())]
  pub(crate) notebook_path: String,
  #[arg(long, default_value_t = default_scan_file_path())]
  pub(crate) scan_file_path: String,
  #[arg(long, default_value_t = default_gitcode_user())]
  pub(crate) gitcode_user: String,
  #[arg(long)]
  pub(crate) auth_cache: Option<String>,
  #[arg(long, default_value_t = config::env_f64(&["JUD_AUTH_REFRESH_MARGIN"], 300.0))]
  pub(crate) auth_refresh_margin: f64,
  #[arg(long, default_value = "notebookcann")]
  pub(crate) notebook_target_contains: String,
  #[arg(long, default_value = "/lab")]
  pub(crate) notebook_page_contains: String,
  #[arg(long, default_value_t = config::default_chrome_bin())]
  pub(crate) chrome_bin: String,
  #[arg(long)]
  pub(crate) chrome_user_data_dir: Option<String>,
  #[arg(long, default_value_t = default_profile_directory())]
  pub(crate) profile_directory: String,
  #[arg(long, action = ArgAction::SetTrue)]
  pub(crate) headless: bool,
  #[arg(long, action = ArgAction::SetTrue)]
  pub(crate) visible: bool,
  #[arg(long, action = ArgAction::SetTrue)]
  pub(crate) no_login_window: bool,
  #[arg(long, default_value_t = 300.0)]
  pub(crate) login_timeout: f64,
  #[arg(long, default_value_t = 3.0)]
  pub(crate) login_probe_interval: f64,
  #[arg(long, default_value = "1440,1000")]
  pub(crate) window_size: String,
  #[arg(long)]
  pub(crate) cdp_port: Option<u16>,
  #[arg(long)]
  pub(crate) cdp_list_url: Option<String>,
  #[arg(long, default_value_t = config::env_string(&["JUD_LISTEN_HOST"], config::DEFAULT_LISTEN_HOST))]
  pub(crate) listen_host: String,
  #[arg(long)]
  pub(crate) listen_port: Option<u16>,
  #[arg(long, default_value_t = config::env_string(&["JUD_STREAM_HOST"], config::DEFAULT_STREAM_HOST))]
  pub(crate) stream_host: String,
  #[arg(long)]
  pub(crate) stream_port: Option<u16>,
  #[arg(long, default_value_t = 4)]
  pub(crate) worker_threads: usize,
  #[arg(long, default_value_t = 600.0)]
  pub(crate) job_retention: f64,
  #[arg(long, default_value_t = 60.0)]
  pub(crate) interval: f64,
  #[arg(long, action = ArgAction::SetTrue)]
  pub(crate) once: bool,
  #[arg(long, action = ArgAction::SetTrue)]
  pub(crate) status_only: bool,
  #[arg(long, action = ArgAction::SetTrue)]
  pub(crate) login: bool,
  #[arg(long, action = ArgAction::SetTrue)]
  pub(crate) no_launch: bool,
  #[arg(long, default_value_t = 8.0)]
  pub(crate) context_wait: f64,
  #[arg(long, default_value_t = 20.0)]
  pub(crate) probe_timeout: f64,
  #[arg(long, default_value_t = 30.0)]
  pub(crate) insert_timeout: f64,
  #[arg(long, default_value_t = 30.0)]
  pub(crate) direct_timeout: f64,
  #[arg(long, default_value_t = 180.0)]
  pub(crate) create_timeout: f64,
  #[arg(long, default_value_t = 5.0)]
  pub(crate) create_probe_interval: f64,
  #[arg(long, default_value_t = 20.0)]
  pub(crate) chrome_start_timeout: f64,
  #[arg(long, default_value_t = 3.0)]
  pub(crate) hub_load_delay: f64,
  #[arg(long)]
  pub(crate) state_file: Option<String>,
  #[arg(long, action = ArgAction::SetTrue)]
  pub(crate) debug: bool,
}

impl Args {
  pub(crate) fn headless(&self) -> bool {
    !self.visible
  }

  pub(crate) fn auth_cache_path(&self) -> std::path::PathBuf {
    if self.auth_cache.is_none() && self.account == config::DEFAULT_ACCOUNT {
      if let Ok(value) = std::env::var("JUD_AUTH_CACHE")
        && !value.is_empty()
      {
        return config::expand_tilde(value);
      }
    }
    self
      .auth_cache
      .as_deref()
      .map(config::expand_tilde)
      .unwrap_or_else(|| {
        config::account_auth_cache(&self.account)
          .unwrap_or_else(|_| config::expand_tilde(config::default_auth_cache()))
      })
  }

  pub(crate) fn chrome_user_data_path(&self) -> std::path::PathBuf {
    if self.chrome_user_data_dir.is_none() && self.account == config::DEFAULT_ACCOUNT {
      if let Ok(value) = std::env::var("JUD_CHROME_PROFILE_DIR")
        && !value.is_empty()
      {
        return config::expand_tilde(value);
      }
    }
    self
      .chrome_user_data_dir
      .as_deref()
      .map(config::expand_tilde)
      .unwrap_or_else(|| {
        config::account_chrome_profile(&self.account)
          .unwrap_or_else(|_| config::expand_tilde(config::default_chrome_profile()))
      })
  }

  pub(crate) fn state_file_path(&self) -> std::path::PathBuf {
    if self.state_file.is_none() && self.account == config::DEFAULT_ACCOUNT {
      if let Ok(value) = std::env::var("JUD_STATE_FILE")
        && !value.is_empty()
      {
        return config::expand_tilde(value);
      }
    }
    self
      .state_file
      .as_deref()
      .map(config::expand_tilde)
      .unwrap_or_else(|| {
        config::account_state_file(&self.account)
          .unwrap_or_else(|_| config::expand_tilde(config::default_state_file()))
      })
  }

  pub(crate) fn cdp_list_url(&self) -> String {
    self.cdp_list_url.clone().unwrap_or_else(|| {
      if let Ok(value) = std::env::var("JUD_CDP_LIST_URL")
        && !value.is_empty()
      {
        return value;
      }
      format!("http://127.0.0.1:{}/json", self.cdp_port())
    })
  }

  pub(crate) fn cdp_port(&self) -> u16 {
    self
      .cdp_port
      .or_else(|| config::account_cdp_port(&self.account).ok())
      .unwrap_or(config::CDP_PORT_POOL_START)
  }

  pub(crate) fn listen_port(&self) -> u16 {
    self
      .listen_port
      .or_else(|| config::account_listen_port(&self.account).ok())
      .unwrap_or(config::DEFAULT_LISTEN_PORT)
  }

  pub(crate) fn stream_port(&self) -> u16 {
    self
      .stream_port
      .or_else(|| config::account_stream_port(&self.account).ok())
      .unwrap_or(config::DEFAULT_STREAM_PORT)
  }
}
