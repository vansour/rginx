use std::env;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};

use rginx_control_types::NodeLifecycleState;

#[derive(Debug, Clone)]
pub struct NodeAgentConfig {
    pub node_id: String,
    pub cluster_id: String,
    pub advertise_addr: String,
    pub role: String,
    pub running_version: String,
    pub control_plane_origin: String,
    pub control_plane_agent_token: String,
    pub dns_udp_bind_addr: Option<SocketAddr>,
    pub dns_tcp_bind_addr: Option<SocketAddr>,
    pub admin_socket_path: PathBuf,
    pub rginx_binary_path: PathBuf,
    pub config_path: PathBuf,
    pub config_backup_dir: PathBuf,
    pub config_staging_dir: PathBuf,
    pub lifecycle_state: NodeLifecycleState,
    pub heartbeat_interval: Duration,
    pub task_poll_interval: Duration,
    pub request_timeout: Duration,
}

impl NodeAgentConfig {
    pub fn from_env() -> Result<Self> {
        let heartbeat_interval = Duration::from_secs(
            env::var("RGINX_NODE_AGENT_HEARTBEAT_SECS")
                .unwrap_or_else(|_| "10".to_string())
                .parse()
                .context("RGINX_NODE_AGENT_HEARTBEAT_SECS should be a positive integer")?,
        );
        let request_timeout = Duration::from_secs(
            env::var("RGINX_NODE_AGENT_REQUEST_TIMEOUT_SECS")
                .unwrap_or_else(|_| "5".to_string())
                .parse()
                .context("RGINX_NODE_AGENT_REQUEST_TIMEOUT_SECS should be a positive integer")?,
        );
        let task_poll_interval = Duration::from_secs(
            env::var("RGINX_NODE_AGENT_TASK_POLL_SECS")
                .unwrap_or_else(|_| "3".to_string())
                .parse()
                .context("RGINX_NODE_AGENT_TASK_POLL_SECS should be a positive integer")?,
        );
        let dns_udp_bind_addr = optional_socket_addr_from_env("RGINX_NODE_DNS_UDP_ADDR")?;
        let dns_tcp_bind_addr = optional_socket_addr_from_env("RGINX_NODE_DNS_TCP_ADDR")?;

        Ok(Self {
            node_id: env::var("RGINX_NODE_ID").unwrap_or_else(|_| "edge-dev-01".to_string()),
            cluster_id: env::var("RGINX_CLUSTER_ID")
                .unwrap_or_else(|_| "cluster-mainland".to_string()),
            advertise_addr: env::var("RGINX_NODE_ADVERTISE_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:8443".to_string()),
            role: env::var("RGINX_NODE_ROLE").unwrap_or_else(|_| "edge".to_string()),
            running_version: env::var("RGINX_NODE_RUNNING_VERSION")
                .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string()),
            control_plane_origin: env::var("RGINX_CONTROL_PLANE_ORIGIN")
                .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string())
                .trim_end_matches('/')
                .to_string(),
            control_plane_agent_token: env::var("RGINX_CONTROL_AGENT_SHARED_TOKEN")
                .context("RGINX_CONTROL_AGENT_SHARED_TOKEN is required")?,
            dns_udp_bind_addr,
            dns_tcp_bind_addr,
            admin_socket_path: env::var("RGINX_ADMIN_SOCKET")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/run/rginx/admin.sock")),
            rginx_binary_path: env::var("RGINX_NODE_BINARY")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/usr/sbin/rginx")),
            config_path: env::var("RGINX_NODE_CONFIG_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/etc/rginx/rginx.ron")),
            config_backup_dir: env::var("RGINX_NODE_CONFIG_BACKUP_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/var/lib/rginx-node-agent/backups")),
            config_staging_dir: env::var("RGINX_NODE_CONFIG_STAGING_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("/var/lib/rginx-node-agent/staging")),
            lifecycle_state: env::var("RGINX_NODE_LIFECYCLE_STATE")
                .unwrap_or_else(|_| "online".to_string())
                .parse()
                .map_err(|error: String| anyhow::anyhow!(error))?,
            heartbeat_interval,
            task_poll_interval,
            request_timeout,
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
