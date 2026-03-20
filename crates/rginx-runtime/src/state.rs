use std::sync::Arc;

use rginx_core::ConfigSnapshot;

#[derive(Clone)]
pub struct RuntimeState {
    pub config: Arc<ConfigSnapshot>,
}

impl RuntimeState {
    pub fn new(config: ConfigSnapshot) -> Self {
        Self { config: Arc::new(config) }
    }
}
