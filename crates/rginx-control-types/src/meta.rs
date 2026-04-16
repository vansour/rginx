use serde::{Deserialize, Serialize};

pub const CONTROL_API_VERSION: &str = "v1alpha1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceHealth {
    pub service: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlaneMeta {
    pub service_name: String,
    pub api_version: String,
    pub api_listen_addr: String,
    pub ui_path: String,
    pub node_agent_path: String,
}
