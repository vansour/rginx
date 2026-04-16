use rginx_control_types::ControlPlaneMeta;

use crate::ControlPlaneServiceConfig;

#[derive(Debug, Clone)]
pub struct MetaService {
    config: ControlPlaneServiceConfig,
}

impl MetaService {
    pub fn new(config: ControlPlaneServiceConfig) -> Self {
        Self { config }
    }

    pub fn get_meta(&self, api_listen_addr: String) -> ControlPlaneMeta {
        ControlPlaneMeta {
            service_name: self.config.service_name.clone(),
            api_version: self.config.api_version.clone(),
            api_listen_addr,
            ui_path: self.config.ui_path.clone(),
            node_agent_path: self.config.node_agent_path.clone(),
        }
    }
}
