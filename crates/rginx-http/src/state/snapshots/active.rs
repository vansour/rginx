use std::sync::Arc;

use rginx_core::ConfigSnapshot;

use crate::proxy::ProxyClients;

#[derive(Clone)]
pub struct ActiveState {
    pub revision: u64,
    pub config: Arc<ConfigSnapshot>,
    pub clients: ProxyClients,
}
