use std::env;
use std::time::Duration;

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct ControlWorkerConfig {
    pub poll_interval: Duration,
}

impl ControlWorkerConfig {
    pub fn from_env() -> Result<Self> {
        let poll_interval_secs: u64 = env::var("RGINX_CONTROL_WORKER_POLL_INTERVAL_SECS")
            .unwrap_or_else(|_| "5".to_string())
            .parse()
            .context("RGINX_CONTROL_WORKER_POLL_INTERVAL_SECS should be a positive integer")?;
        if poll_interval_secs == 0 {
            bail!("RGINX_CONTROL_WORKER_POLL_INTERVAL_SECS should be a positive integer");
        }

        Ok(Self { poll_interval: Duration::from_secs(poll_interval_secs) })
    }
}
