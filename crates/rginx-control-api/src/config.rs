use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, Result};

#[derive(Debug, Clone)]
pub struct ControlApiConfig {
    pub bind_addr: SocketAddr,
    pub agent_shared_token: String,
    pub ui_dir: Option<PathBuf>,
}

impl ControlApiConfig {
    pub fn from_env() -> Result<Self> {
        let bind_addr = env::var("RGINX_CONTROL_API_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
            .parse()
            .context("RGINX_CONTROL_API_ADDR should be a valid socket address")?;
        let agent_shared_token = env::var("RGINX_CONTROL_AGENT_SHARED_TOKEN")
            .context("RGINX_CONTROL_AGENT_SHARED_TOKEN is required")?;
        let ui_dir = match env::var("RGINX_CONTROL_UI_DIR") {
            Ok(value) if value.trim().is_empty() => None,
            Ok(value) => Some(PathBuf::from(value)),
            Err(_) => Some(PathBuf::from("/opt/rginx/control-console")),
        };

        Ok(Self { bind_addr, agent_shared_token, ui_dir })
    }
}
