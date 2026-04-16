use std::env;
use std::time::Duration;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct ControlWorkerConfig {
    pub concurrency: usize,
    pub poll_interval: Duration,
}

impl ControlWorkerConfig {
    pub fn from_env() -> Result<Self> {
        let concurrency = env::var("RGINX_CONTROL_WORKER_CONCURRENCY")
            .unwrap_or_else(|_| "2".to_string())
            .parse()
            .context("RGINX_CONTROL_WORKER_CONCURRENCY should be a positive integer")?;
        let poll_interval_secs = env::var("RGINX_CONTROL_WORKER_POLL_INTERVAL_SECS")
            .unwrap_or_else(|_| "5".to_string())
            .parse()
            .context("RGINX_CONTROL_WORKER_POLL_INTERVAL_SECS should be a positive integer")?;

        Ok(Self { concurrency, poll_interval: Duration::from_secs(poll_interval_secs) })
    }
}
