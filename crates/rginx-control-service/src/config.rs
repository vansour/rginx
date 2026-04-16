use std::{env, time::Duration};

use anyhow::{Context, Result};

use rginx_control_types::CONTROL_API_VERSION;

#[derive(Debug, Clone)]
pub struct ControlPlaneAuthConfig {
    pub session_secret: String,
    pub session_ttl: Duration,
}

#[derive(Debug, Clone)]
pub struct ControlPlaneServiceConfig {
    pub service_name: String,
    pub api_version: String,
    pub ui_path: String,
    pub node_agent_path: String,
    pub node_offline_threshold: Duration,
    pub auth: Option<ControlPlaneAuthConfig>,
}

impl Default for ControlPlaneServiceConfig {
    fn default() -> Self {
        Self {
            service_name: "rginx-control-api".to_string(),
            api_version: CONTROL_API_VERSION.to_string(),
            ui_path: "/".to_string(),
            node_agent_path: "rginx-node-agent".to_string(),
            node_offline_threshold: Duration::from_secs(30),
            auth: None,
        }
    }
}

impl ControlPlaneServiceConfig {
    pub fn for_api() -> Result<Self> {
        Ok(Self {
            node_offline_threshold: Duration::from_secs(
                env::var("RGINX_CONTROL_NODE_OFFLINE_THRESHOLD_SECS")
                    .unwrap_or_else(|_| "30".to_string())
                    .parse()
                    .context("RGINX_CONTROL_NODE_OFFLINE_THRESHOLD_SECS should be a valid u64")?,
            ),
            auth: Some(ControlPlaneAuthConfig {
                session_secret: env::var("RGINX_CONTROL_AUTH_SESSION_SECRET")
                    .context("RGINX_CONTROL_AUTH_SESSION_SECRET is required")?,
                session_ttl: Duration::from_secs(
                    env::var("RGINX_CONTROL_AUTH_SESSION_TTL_SECS")
                        .unwrap_or_else(|_| "86400".to_string())
                        .parse()
                        .context("RGINX_CONTROL_AUTH_SESSION_TTL_SECS should be a valid u64")?,
                ),
            }),
            ..Self::default()
        })
    }

    pub fn for_worker() -> Self {
        let mut config =
            Self { service_name: "rginx-control-worker".to_string(), ..Self::default() };
        if let Ok(value) = env::var("RGINX_CONTROL_NODE_OFFLINE_THRESHOLD_SECS")
            && let Ok(seconds) = value.parse::<u64>()
        {
            config.node_offline_threshold = Duration::from_secs(seconds);
        }
        config
    }
}
