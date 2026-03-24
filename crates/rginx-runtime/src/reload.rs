use std::fs;
use std::sync::Arc;

use rginx_core::{ConfigSnapshot, Result};

use crate::state::RuntimeState;

pub async fn reload(state: &RuntimeState) -> Result<Arc<ConfigSnapshot>> {
    let config_source = fs::read_to_string(&state.config_path)?;
    let next = rginx_config::load_and_compile_from_str(&config_source, &state.config_path)?;
    state.http.replace_with_source(next, config_source).await
}
