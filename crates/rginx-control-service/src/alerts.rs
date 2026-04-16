use rginx_control_store::ControlPlaneStore;
use rginx_control_types::{
    AlertSeverity, ControlPlaneAlertSummary, DeploymentStatus, NodeLifecycleState,
};

use crate::{ServiceError, ServiceResult};

#[derive(Debug, Clone)]
pub struct AlertService {
    store: ControlPlaneStore,
}

impl AlertService {
    pub fn new(store: ControlPlaneStore) -> Self {
        Self { store }
    }

    pub async fn list_current_alerts(&self) -> ServiceResult<Vec<ControlPlaneAlertSummary>> {
        let nodes = self
            .store
            .node_repository()
            .list_nodes()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let deployments = self
            .store
            .deployment_repository()
            .list_deployments()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let mut alerts = Vec::new();

        for node in nodes {
            match node.state {
                NodeLifecycleState::Offline => alerts.push(ControlPlaneAlertSummary {
                    alert_id: format!("alert_node_offline_{}", node.node_id),
                    cluster_id: Some(node.cluster_id.clone()),
                    severity: AlertSeverity::Critical,
                    kind: "node_offline".to_string(),
                    title: format!("Node `{}` is offline", node.node_id),
                    message: node
                        .status_reason
                        .clone()
                        .unwrap_or_else(|| "node stopped reporting heartbeats".to_string()),
                    resource_type: "node".to_string(),
                    resource_id: node.node_id.clone(),
                    observed_at_unix_ms: node.last_seen_unix_ms,
                }),
                NodeLifecycleState::Drifted => alerts.push(ControlPlaneAlertSummary {
                    alert_id: format!("alert_node_drifted_{}", node.node_id),
                    cluster_id: Some(node.cluster_id.clone()),
                    severity: AlertSeverity::Warning,
                    kind: "node_drifted".to_string(),
                    title: format!("Node `{}` is drifted", node.node_id),
                    message: node.status_reason.clone().unwrap_or_else(|| {
                        "agent is online but runtime diagnostics are unavailable".to_string()
                    }),
                    resource_type: "node".to_string(),
                    resource_id: node.node_id.clone(),
                    observed_at_unix_ms: node.last_seen_unix_ms,
                }),
                _ => {}
            }
        }

        for deployment in deployments {
            match deployment.status {
                DeploymentStatus::Failed => alerts.push(ControlPlaneAlertSummary {
                    alert_id: format!("alert_deployment_failed_{}", deployment.deployment_id),
                    cluster_id: Some(deployment.cluster_id.clone()),
                    severity: AlertSeverity::Critical,
                    kind: "deployment_failed".to_string(),
                    title: format!("Deployment `{}` failed", deployment.deployment_id),
                    message: deployment.status_reason.clone().unwrap_or_else(|| {
                        format!(
                            "revision `{}` failed on {} target(s)",
                            deployment.revision_version_label, deployment.failed_nodes
                        )
                    }),
                    resource_type: "deployment".to_string(),
                    resource_id: deployment.deployment_id.clone(),
                    observed_at_unix_ms: deployment
                        .finished_at_unix_ms
                        .unwrap_or(deployment.created_at_unix_ms),
                }),
                DeploymentStatus::Paused => alerts.push(ControlPlaneAlertSummary {
                    alert_id: format!("alert_deployment_paused_{}", deployment.deployment_id),
                    cluster_id: Some(deployment.cluster_id.clone()),
                    severity: AlertSeverity::Warning,
                    kind: "deployment_paused".to_string(),
                    title: format!("Deployment `{}` is paused", deployment.deployment_id),
                    message: deployment.status_reason.clone().unwrap_or_else(|| {
                        "deployment is paused and requires operator attention".to_string()
                    }),
                    resource_type: "deployment".to_string(),
                    resource_id: deployment.deployment_id.clone(),
                    observed_at_unix_ms: deployment
                        .started_at_unix_ms
                        .unwrap_or(deployment.created_at_unix_ms),
                }),
                DeploymentStatus::Running if deployment.status_reason.is_some() => {
                    alerts.push(ControlPlaneAlertSummary {
                        alert_id: format!(
                            "alert_deployment_running_warn_{}",
                            deployment.deployment_id
                        ),
                        cluster_id: Some(deployment.cluster_id.clone()),
                        severity: AlertSeverity::Warning,
                        kind: "deployment_attention".to_string(),
                        title: format!(
                            "Deployment `{}` requires attention",
                            deployment.deployment_id
                        ),
                        message: deployment.status_reason.clone().unwrap_or_default(),
                        resource_type: "deployment".to_string(),
                        resource_id: deployment.deployment_id.clone(),
                        observed_at_unix_ms: deployment
                            .started_at_unix_ms
                            .unwrap_or(deployment.created_at_unix_ms),
                    })
                }
                _ => {}
            }
        }

        alerts.sort_by(|left, right| {
            severity_rank(right.severity)
                .cmp(&severity_rank(left.severity))
                .then(right.observed_at_unix_ms.cmp(&left.observed_at_unix_ms))
                .then(left.alert_id.cmp(&right.alert_id))
        });

        Ok(alerts)
    }
}

fn severity_rank(severity: AlertSeverity) -> u8 {
    match severity {
        AlertSeverity::Critical => 2,
        AlertSeverity::Warning => 1,
    }
}
