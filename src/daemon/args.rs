use crate::config;
use clap::{ArgAction, Parser};

fn default_hub_url() -> String {
  config::DEFAULT_HUB_URL.to_string()
}

fn default_repo_url() -> String {
  config::env_string(
    &["GJTD_REPO_URL", "JUPYTERD_REPO_URL"],
    config::DEFAULT_REPO_URL,
  )
}

fn default_ttl() -> String {
  config::env_string(&["GJTD_TTL", "JUPYTERD_TTL"], "120")
}

fn default_disk_size() -> String {
  config::env_string(&["GJTD_DISK_SIZE", "JUPYTERD_DISK_SIZE"], "40Gi")
}

fn default_notebook_path() -> String {
  config::env_string(
    &["GJTD_NOTEBOOK_PATH", "JUPYTERD_NOTEBOOK_PATH"],
    config::DEFAULT_NOTEBOOK_PATH,
  )
}

fn default_scan_file_path() -> String {
  config::env_string(
    &["GJTD_SCAN_FILE_PATH", "JUPYTERD_SCAN_FILE_PATH"],
    config::DEFAULT_SCAN_FILE_PATH,
  )
}

fn default_gitcode_user() -> String {
  config::env_string(&["GITCODE_USER"], "username")
}

fn default_auth_cache() -> String {
  config::env_string(
    &["GJTD_AUTH_CACHE", "JUPYTERD_AUTH_CACHE"],
    &config::default_auth_cache(),
  )
}

fn default_chrome_profile() -> String {
  config::env_string(
    &["GJTD_CHROME_PROFILE_DIR", "JUPYTERD_CHROME_PROFILE_DIR"],
    &config::default_chrome_profile(),
  )
}

fn default_profile_directory() -> String {
  config::env_string(
    &["GJTD_CHROME_PROFILE", "JUPYTERD_CHROME_PROFILE"],
    "Default",
  )
}

fn default_cdp_list_url() -> String {
  config::env_string(
    &["GJTD_CDP_LIST_URL", "JUPYTER_SH_CDP_LIST_URL"],
    config::DEFAULT_CDP_LIST_URL,
  )
}

fn default_state_file() -> String {
  config::env_string(
    &["GJTD_STATE_FILE", "JUPYTERD_STATE_FILE"],
    config::DEFAULT_STATE_FILE,
  )
}

#[derive(Clone, Parser)]
#[command(
  name = "gjtd",
  about = "Maintain an available GitCode CANN JupyterLab notebook instance."
)]
pub(crate) struct Args {
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
  #[arg(long, default_value_t = default_auth_cache())]
  pub(crate) auth_cache: String,
  #[arg(long, default_value_t = config::env_f64(&["GJTD_AUTH_REFRESH_MARGIN", "JUPYTERD_AUTH_REFRESH_MARGIN"], 300.0))]
  pub(crate) auth_refresh_margin: f64,
  #[arg(long, default_value = "notebookcann")]
  pub(crate) notebook_target_contains: String,
  #[arg(long, default_value = "/lab")]
  pub(crate) notebook_page_contains: String,
  #[arg(long, default_value_t = config::default_chrome_bin())]
  pub(crate) chrome_bin: String,
  #[arg(long, default_value_t = default_chrome_profile())]
  pub(crate) chrome_user_data_dir: String,
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
  #[arg(long, default_value_t = config::env_u16(&["GJTD_CDP_PORT", "JUPYTERD_CDP_PORT"], 9222))]
  pub(crate) cdp_port: u16,
  #[arg(long, default_value_t = default_cdp_list_url())]
  pub(crate) cdp_list_url: String,
  #[arg(long, default_value_t = config::env_string(&["GJTD_LISTEN_HOST", "JUPYTERD_LISTEN_HOST"], config::DEFAULT_LISTEN_HOST))]
  pub(crate) listen_host: String,
  #[arg(long, default_value_t = config::env_u16(&["GJTD_LISTEN_PORT", "JUPYTERD_LISTEN_PORT"], config::DEFAULT_LISTEN_PORT))]
  pub(crate) listen_port: u16,
  #[arg(long, default_value_t = config::env_string(&["GJTD_STREAM_HOST", "JUPYTERD_STREAM_HOST"], config::DEFAULT_STREAM_HOST))]
  pub(crate) stream_host: String,
  #[arg(long, default_value_t = config::env_u16(&["GJTD_STREAM_PORT", "JUPYTERD_STREAM_PORT"], config::DEFAULT_STREAM_PORT))]
  pub(crate) stream_port: u16,
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
  #[arg(long, default_value_t = default_state_file())]
  pub(crate) state_file: String,
  #[arg(long, action = ArgAction::SetTrue)]
  pub(crate) debug: bool,
}

impl Args {
  pub(crate) fn headless(&self) -> bool {
    !self.visible
  }
}
