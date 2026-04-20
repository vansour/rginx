use std::{env, time::Duration};

use anyhow::{Context, Result, bail};

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
            service_name: "rginx-web".to_string(),
            api_version: CONTROL_API_VERSION.to_string(),
            ui_path: "/".to_string(),
            node_agent_path: "rginx-node-agent".to_string(),
            node_offline_threshold: Duration::from_secs(30),
            auth: None,
        }
    }
}

impl ControlPlaneServiceConfig {
    pub fn for_web() -> Result<Self> {
        let session_secret = env::var("RGINX_CONTROL_AUTH_SESSION_SECRET")
            .context("RGINX_CONTROL_AUTH_SESSION_SECRET is required")?;
        if session_secret.trim().is_empty() {
            bail!("RGINX_CONTROL_AUTH_SESSION_SECRET should not be empty");
        }

        let session_ttl_secs = env::var("RGINX_CONTROL_AUTH_SESSION_TTL_SECS")
            .unwrap_or_else(|_| "86400".to_string())
            .parse()
            .context("RGINX_CONTROL_AUTH_SESSION_TTL_SECS should be a valid u64")?;
        if session_ttl_secs == 0 {
            bail!("RGINX_CONTROL_AUTH_SESSION_TTL_SECS should be greater than zero");
        }

        Ok(Self {
            node_offline_threshold: Duration::from_secs(
                env::var("RGINX_CONTROL_NODE_OFFLINE_THRESHOLD_SECS")
                    .unwrap_or_else(|_| "30".to_string())
                    .parse()
                    .context("RGINX_CONTROL_NODE_OFFLINE_THRESHOLD_SECS should be a valid u64")?,
            ),
            auth: Some(ControlPlaneAuthConfig {
                session_secret: session_secret.trim().to_string(),
                session_ttl: Duration::from_secs(session_ttl_secs),
            }),
            ..Self::default()
        })
    }
}
