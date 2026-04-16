use serde::{Deserialize, Serialize};

use crate::{
    ControlPlaneAlertSummary, DashboardSummary, DeploymentDetail, NodeDetailResponse, NodeSummary,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlaneOverviewEvent {
    pub event_id: String,
    pub emitted_at_unix_ms: u64,
    pub dashboard: DashboardSummary,
    pub nodes: Vec<NodeSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlaneNodeDetailEvent {
    pub event_id: String,
    pub emitted_at_unix_ms: u64,
    pub detail: NodeDetailResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlaneDeploymentEvent {
    pub event_id: String,
    pub emitted_at_unix_ms: u64,
    pub detail: DeploymentDetail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlPlaneAlertsEvent {
    pub event_id: String,
    pub emitted_at_unix_ms: u64,
    pub alerts: Vec<ControlPlaneAlertSummary>,
}
