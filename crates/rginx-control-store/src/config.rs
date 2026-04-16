use std::env;

use anyhow::{Context, Result};
use sqlx::postgres::PgConnectOptions;

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
        }
    }
}

impl ControlPlaneStoreConfig {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            db_host: env::var("RGINX_CONTROL_DB_HOST").unwrap_or_else(|_| Self::default().db_host),
            db_port: env::var("RGINX_CONTROL_DB_PORT")
                .unwrap_or_else(|_| Self::default().db_port.to_string())
                .parse()
                .context("RGINX_CONTROL_DB_PORT should be a valid u16")?,
            db_user: env::var("RGINX_CONTROL_DB_USER").unwrap_or_else(|_| Self::default().db_user),
            db_password: env::var("RGINX_CONTROL_DB_PASSWORD")
                .unwrap_or_else(|_| Self::default().db_password),
            db_name: env::var("RGINX_CONTROL_DB_NAME").unwrap_or_else(|_| Self::default().db_name),
            db_max_connections: env::var("RGINX_CONTROL_DB_MAX_CONNECTIONS")
                .unwrap_or_else(|_| Self::default().db_max_connections.to_string())
                .parse()
                .context("RGINX_CONTROL_DB_MAX_CONNECTIONS should be a valid u32")?,
            dragonfly_host: env::var("RGINX_CONTROL_DRAGONFLY_HOST")
                .unwrap_or_else(|_| Self::default().dragonfly_host),
            dragonfly_port: env::var("RGINX_CONTROL_DRAGONFLY_PORT")
                .unwrap_or_else(|_| Self::default().dragonfly_port.to_string())
                .parse()
                .context("RGINX_CONTROL_DRAGONFLY_PORT should be a valid u16")?,
            dragonfly_key_prefix: env::var("RGINX_CONTROL_DRAGONFLY_KEY_PREFIX")
                .unwrap_or_else(|_| Self::default().dragonfly_key_prefix),
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
