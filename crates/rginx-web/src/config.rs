use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone)]
pub struct RginxWebConfig {
    pub bind_addr: SocketAddr,
    pub poll_interval: Duration,
    pub agent_shared_token: String,
    pub ui_dir: Option<PathBuf>,
    pub dns_udp_bind_addr: Option<SocketAddr>,
    pub dns_tcp_bind_addr: Option<SocketAddr>,
}

impl RginxWebConfig {
    pub fn from_env() -> Result<Self> {
        let bind_addr = env::var("RGINX_CONTROL_API_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:8080".to_string())
            .parse()
            .context("RGINX_CONTROL_API_ADDR should be a valid socket address")?;
        let poll_interval_secs: u64 = env::var("RGINX_CONTROL_WORKER_POLL_INTERVAL_SECS")
            .unwrap_or_else(|_| "5".to_string())
            .parse()
            .context("RGINX_CONTROL_WORKER_POLL_INTERVAL_SECS should be a positive integer")?;
        if poll_interval_secs == 0 {
            bail!("RGINX_CONTROL_WORKER_POLL_INTERVAL_SECS should be a positive integer");
        }
        let agent_shared_token = env::var("RGINX_CONTROL_AGENT_SHARED_TOKEN")
            .context("RGINX_CONTROL_AGENT_SHARED_TOKEN is required")?;
        let ui_dir = match env::var("RGINX_CONTROL_UI_DIR") {
            Ok(value) if value.trim().is_empty() => None,
            Ok(value) => Some(PathBuf::from(value)),
            Err(_) => None,
        };
        let dns_udp_bind_addr = optional_socket_addr_from_env("RGINX_CONTROL_DNS_UDP_ADDR")?;
        let dns_tcp_bind_addr = optional_socket_addr_from_env("RGINX_CONTROL_DNS_TCP_ADDR")?;

        Ok(Self {
            bind_addr,
            poll_interval: Duration::from_secs(poll_interval_secs),
            agent_shared_token,
            ui_dir,
            dns_udp_bind_addr,
            dns_tcp_bind_addr,
        })
    }
}

fn optional_socket_addr_from_env(name: &str) -> Result<Option<SocketAddr>> {
    match env::var(name) {
        Ok(value) if value.trim().is_empty() => Ok(None),
        Ok(value) => value
            .parse()
            .with_context(|| format!("{name} should be a valid socket address"))
            .map(Some),
        Err(_) => Ok(None),
    }
}
