use std::sync::Arc;

use rginx_core::{ConfigSnapshot, Result};

use crate::state::RuntimeState;

pub async fn reload(state: &RuntimeState) -> Result<Arc<ConfigSnapshot>> {
    let config = rginx_config::load_and_compile(&state.config_path)?;
    state.http.replace(config).await
}
