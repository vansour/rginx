use std::collections::HashMap;
use std::sync::Arc;

use super::{Listener, RuntimeSettings, Upstream, VirtualHost};

#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    pub runtime: RuntimeSettings,
    pub listeners: Vec<Listener>,
    pub default_vhost: VirtualHost,
    pub vhosts: Vec<VirtualHost>,
    pub upstreams: HashMap<String, Arc<Upstream>>,
}

impl ConfigSnapshot {
    pub fn total_route_count(&self) -> usize {
        self.default_vhost.routes.len()
            + self.vhosts.iter().map(|vhost| vhost.routes.len()).sum::<usize>()
    }

    pub fn total_vhost_count(&self) -> usize {
        1 + self.vhosts.len()
    }

    pub fn total_listener_count(&self) -> usize {
        self.listeners.len()
    }

    pub fn total_listener_binding_count(&self) -> usize {
        self.listeners.iter().map(Listener::binding_count).sum()
    }

    pub fn tls_enabled(&self) -> bool {
        self.listeners.iter().any(Listener::tls_enabled)
    }

    pub fn http3_enabled(&self) -> bool {
        self.listeners.iter().any(Listener::http3_enabled)
    }

    pub fn listener(&self, id: &str) -> Option<&Listener> {
        self.listeners.iter().find(|listener| listener.id == id)
    }
}
