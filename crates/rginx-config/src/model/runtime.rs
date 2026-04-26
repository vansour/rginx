use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeConfig {
    pub shutdown_timeout_secs: u64,
    #[serde(default)]
    pub worker_threads: Option<u64>,
    #[serde(default)]
    pub accept_workers: Option<u64>,
}
