use std::path::PathBuf;
use std::sync::Arc;

use rginx_core::{ConfigSnapshot, Result};

#[derive(Clone)]
pub struct RuntimeState {
    pub config_path: PathBuf,
    pub http: rginx_http::SharedState,
}

impl RuntimeState {
    pub fn new(config_path: PathBuf, config: ConfigSnapshot) -> Result<Self> {
        Ok(Self {
            config_path: config_path.clone(),
            http: rginx_http::SharedState::from_config_path(config_path, config)?,
        })
    }

    pub async fn current_config(&self) -> Arc<ConfigSnapshot> {
        self.http.current_config().await
    }
}
