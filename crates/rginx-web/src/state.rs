use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use rginx_control_service::ControlPlaneServices;

use crate::dns_runtime::DnsRuntimeManager;

#[derive(Debug, Clone)]
pub struct AppState {
    api_bind_addr: SocketAddr,
    agent_shared_token: Arc<str>,
    ui_dir: Option<Arc<PathBuf>>,
    services: Arc<ControlPlaneServices>,
    dns_runtime: Option<Arc<DnsRuntimeManager>>,
}

impl AppState {
    pub fn new(
        api_bind_addr: SocketAddr,
        agent_shared_token: String,
        ui_dir: Option<PathBuf>,
        services: ControlPlaneServices,
        dns_runtime: Option<Arc<DnsRuntimeManager>>,
    ) -> Self {
        Self {
            api_bind_addr,
            agent_shared_token: Arc::from(agent_shared_token),
            ui_dir: ui_dir.map(Arc::new),
            services: Arc::new(services),
            dns_runtime,
        }
    }

    pub fn api_bind_addr(&self) -> SocketAddr {
        self.api_bind_addr
    }

    pub fn services(&self) -> &ControlPlaneServices {
        self.services.as_ref()
    }

    pub fn agent_shared_token(&self) -> &str {
        self.agent_shared_token.as_ref()
    }

    pub fn ui_dir(&self) -> Option<PathBuf> {
        self.ui_dir.as_ref().map(|value| value.as_ref().clone())
    }

    pub fn dns_runtime(&self) -> Option<Arc<DnsRuntimeManager>> {
        self.dns_runtime.clone()
    }
}
