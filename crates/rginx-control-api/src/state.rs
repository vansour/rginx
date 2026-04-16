use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use rginx_control_service::ControlPlaneServices;

#[derive(Debug, Clone)]
pub struct AppState {
    api_bind_addr: SocketAddr,
    agent_shared_token: Arc<str>,
    ui_dir: Option<Arc<PathBuf>>,
    services: Arc<ControlPlaneServices>,
}

impl AppState {
    pub fn new(
        api_bind_addr: SocketAddr,
        agent_shared_token: String,
        ui_dir: Option<PathBuf>,
        services: ControlPlaneServices,
    ) -> Self {
        Self {
            api_bind_addr,
            agent_shared_token: Arc::from(agent_shared_token),
            ui_dir: ui_dir.map(Arc::new),
            services: Arc::new(services),
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
}
