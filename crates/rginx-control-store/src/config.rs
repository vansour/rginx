use std::env;

use anyhow::{Context, Result, bail};
use sqlx::postgres::PgConnectOptions;

#[derive(Debug, Clone)]
pub struct BootstrapAdminConfig {
    pub username: String,
    pub password: String,
    pub display_name: String,
}

#[derive(Debug, Clone)]
pub struct ControlPlaneStoreConfig {
    pub db_host: String,
    pub db_port: u16,
    pub db_user: String,
    pub db_password: String,
    pub db_name: String,
    pub db_max_connections: u32,
    pub dragonfly_host: String,
    pub dragonfly_port: u16,
    pub dragonfly_key_prefix: String,
    pub bootstrap_admin: BootstrapAdminConfig,
}

impl Default for ControlPlaneStoreConfig {
    fn default() -> Self {
        Self {
            db_host: "127.0.0.1".to_string(),
            db_port: 5432,
            db_user: "rginx".to_string(),
            db_password: "rginx".to_string(),
            db_name: "rginx_control".to_string(),
            db_max_connections: 10,
            dragonfly_host: "127.0.0.1".to_string(),
            dragonfly_port: 6379,
            dragonfly_key_prefix: "rginx:control".to_string(),
            bootstrap_admin: BootstrapAdminConfig {
                username: "admin".to_string(),
                password: "admin".to_string(),
                display_name: "Local Admin".to_string(),
            },
        }
    }
}

impl ControlPlaneStoreConfig {
    pub fn from_env() -> Result<Self> {
        let defaults = Self::default();
        let bootstrap_admin_username = env::var("RGINX_CONTROL_BOOTSTRAP_ADMIN_USERNAME")
            .unwrap_or_else(|_| defaults.bootstrap_admin.username.clone());
        let bootstrap_admin_password = env::var("RGINX_CONTROL_BOOTSTRAP_ADMIN_PASSWORD")
            .unwrap_or_else(|_| defaults.bootstrap_admin.password.clone());
        let bootstrap_admin_display_name = env::var("RGINX_CONTROL_BOOTSTRAP_ADMIN_DISPLAY_NAME")
            .unwrap_or_else(|_| defaults.bootstrap_admin.display_name.clone());

        if bootstrap_admin_username.trim().is_empty() {
            bail!("RGINX_CONTROL_BOOTSTRAP_ADMIN_USERNAME should not be empty");
        }
        if bootstrap_admin_password.is_empty() {
            bail!("RGINX_CONTROL_BOOTSTRAP_ADMIN_PASSWORD should not be empty");
        }
        if bootstrap_admin_display_name.trim().is_empty() {
            bail!("RGINX_CONTROL_BOOTSTRAP_ADMIN_DISPLAY_NAME should not be empty");
        }

        Ok(Self {
            db_host: env::var("RGINX_CONTROL_DB_HOST").unwrap_or_else(|_| defaults.db_host.clone()),
            db_port: env::var("RGINX_CONTROL_DB_PORT")
                .unwrap_or_else(|_| defaults.db_port.to_string())
                .parse()
                .context("RGINX_CONTROL_DB_PORT should be a valid u16")?,
            db_user: env::var("RGINX_CONTROL_DB_USER").unwrap_or_else(|_| defaults.db_user.clone()),
            db_password: env::var("RGINX_CONTROL_DB_PASSWORD")
                .unwrap_or_else(|_| defaults.db_password.clone()),
            db_name: env::var("RGINX_CONTROL_DB_NAME").unwrap_or_else(|_| defaults.db_name.clone()),
            db_max_connections: env::var("RGINX_CONTROL_DB_MAX_CONNECTIONS")
                .unwrap_or_else(|_| defaults.db_max_connections.to_string())
                .parse()
                .context("RGINX_CONTROL_DB_MAX_CONNECTIONS should be a valid u32")?,
            dragonfly_host: env::var("RGINX_CONTROL_DRAGONFLY_HOST")
                .unwrap_or_else(|_| defaults.dragonfly_host.clone()),
            dragonfly_port: env::var("RGINX_CONTROL_DRAGONFLY_PORT")
                .unwrap_or_else(|_| defaults.dragonfly_port.to_string())
                .parse()
                .context("RGINX_CONTROL_DRAGONFLY_PORT should be a valid u16")?,
            dragonfly_key_prefix: env::var("RGINX_CONTROL_DRAGONFLY_KEY_PREFIX")
                .unwrap_or_else(|_| defaults.dragonfly_key_prefix.clone()),
            bootstrap_admin: BootstrapAdminConfig {
                username: bootstrap_admin_username.trim().to_string(),
                password: bootstrap_admin_password,
                display_name: bootstrap_admin_display_name.trim().to_string(),
            },
        })
    }

    pub fn postgres_endpoint(&self) -> String {
        format!("{}:{}/{}", self.db_host, self.db_port, self.db_name)
    }

    pub fn pg_connect_options(&self) -> PgConnectOptions {
        PgConnectOptions::new()
            .host(&self.db_host)
            .port(self.db_port)
            .username(&self.db_user)
            .password(&self.db_password)
            .database(&self.db_name)
    }

    pub fn dragonfly_endpoint(&self) -> String {
        format!("{}:{}", self.dragonfly_host, self.dragonfly_port)
    }
}
