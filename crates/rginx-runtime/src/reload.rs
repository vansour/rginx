use std::sync::Arc;

use rginx_core::{ConfigSnapshot, Result};

use crate::state::RuntimeState;

pub struct ReloadSuccess {
    pub config: Arc<ConfigSnapshot>,
    pub revision: u64,
}

pub async fn reload(state: &RuntimeState) -> Result<ReloadSuccess> {
    let config = match rginx_config::load_and_compile(&state.config_path) {
        Ok(config) => config,
        Err(error) => {
            state.http.record_reload_failure(error.to_string());
            return Err(error);
        }
    };
    let config = match state.http.replace(config).await {
        Ok(config) => config,
        Err(error) => {
            state.http.record_reload_failure(error.to_string());
            return Err(error);
        }
    };
    let revision = state.http.current_revision().await;
    state.http.record_reload_success(revision);
    Ok(ReloadSuccess { config, revision })
}
