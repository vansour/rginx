use serde::{Deserialize, Serialize};

use crate::{ConfigRevisionSummary, ControlPlaneAlertSummary, DeploymentSummary, NodeSummary};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DashboardSummary {
    pub total_clusters: u32,
    pub total_nodes: u32,
    pub online_nodes: u32,
    pub draining_nodes: u32,
    pub offline_nodes: u32,
    pub drifted_nodes: u32,
    pub total_revisions: u64,
    pub active_deployments: u32,
    pub open_alert_count: u32,
    pub critical_alert_count: u32,
    pub warning_alert_count: u32,
    pub latest_revision: Option<ConfigRevisionSummary>,
    pub recent_nodes: Vec<NodeSummary>,
    pub recent_deployments: Vec<DeploymentSummary>,
    pub open_alerts: Vec<ControlPlaneAlertSummary>,
}
