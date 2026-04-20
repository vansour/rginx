use rginx_control_store::ControlPlaneStore;
use rginx_control_types::{
    AlertSeverity, ControlPlaneAlertSummary, DeploymentStatus, DeploymentSummary,
    DnsDeploymentStatus, DnsDeploymentSummary, NodeLifecycleState, NodeSummary,
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
        let dns_deployments = self
            .store
            .dns_deployment_repository()
            .list_deployments()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;

        Ok(Self::build_alerts(nodes, deployments, dns_deployments))
    }

    fn build_alerts(
        nodes: Vec<NodeSummary>,
        deployments: Vec<DeploymentSummary>,
        dns_deployments: Vec<DnsDeploymentSummary>,
    ) -> Vec<ControlPlaneAlertSummary> {
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
                        "deployment is paused and requires administrator attention".to_string()
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

        for deployment in dns_deployments {
            match deployment.status {
                DnsDeploymentStatus::Failed => alerts.push(ControlPlaneAlertSummary {
                    alert_id: format!("alert_dns_deployment_failed_{}", deployment.deployment_id),
                    cluster_id: Some(deployment.cluster_id.clone()),
                    severity: AlertSeverity::Critical,
                    kind: "dns_deployment_failed".to_string(),
                    title: format!("DNS deployment `{}` failed", deployment.deployment_id),
                    message: deployment.status_reason.clone().unwrap_or_else(|| {
                        format!(
                            "dns revision `{}` failed on {} target(s)",
                            deployment.revision_version_label, deployment.failed_nodes
                        )
                    }),
                    resource_type: "dns_deployment".to_string(),
                    resource_id: deployment.deployment_id.clone(),
                    observed_at_unix_ms: deployment
                        .finished_at_unix_ms
                        .unwrap_or(deployment.created_at_unix_ms),
                }),
                DnsDeploymentStatus::Paused => alerts.push(ControlPlaneAlertSummary {
                    alert_id: format!("alert_dns_deployment_paused_{}", deployment.deployment_id),
                    cluster_id: Some(deployment.cluster_id.clone()),
                    severity: AlertSeverity::Warning,
                    kind: "dns_deployment_paused".to_string(),
                    title: format!("DNS deployment `{}` is paused", deployment.deployment_id),
                    message: deployment.status_reason.clone().unwrap_or_else(|| {
                        "dns deployment is paused and requires administrator attention".to_string()
                    }),
                    resource_type: "dns_deployment".to_string(),
                    resource_id: deployment.deployment_id.clone(),
                    observed_at_unix_ms: deployment
                        .started_at_unix_ms
                        .unwrap_or(deployment.created_at_unix_ms),
                }),
                DnsDeploymentStatus::Running if deployment.status_reason.is_some() => {
                    alerts.push(ControlPlaneAlertSummary {
                        alert_id: format!(
                            "alert_dns_deployment_running_warn_{}",
                            deployment.deployment_id
                        ),
                        cluster_id: Some(deployment.cluster_id.clone()),
                        severity: AlertSeverity::Warning,
                        kind: "dns_deployment_attention".to_string(),
                        title: format!(
                            "DNS deployment `{}` requires attention",
                            deployment.deployment_id
                        ),
                        message: deployment.status_reason.clone().unwrap_or_default(),
                        resource_type: "dns_deployment".to_string(),
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

        alerts
    }
}

fn severity_rank(severity: AlertSeverity) -> u8 {
    match severity {
        AlertSeverity::Critical => 2,
        AlertSeverity::Warning => 1,
    }
}

#[cfg(test)]
mod tests {
    use rginx_control_types::{
        DeploymentStatus, DeploymentSummary, DnsDeploymentStatus, DnsDeploymentSummary,
        NodeLifecycleState, NodeSummary,
    };

    use super::AlertService;

    #[test]
    fn build_alerts_includes_dns_deployment_variants_and_sorts_them() {
        let alerts = AlertService::build_alerts(
            vec![sample_node(
                "edge-offline-01",
                NodeLifecycleState::Offline,
                Some("heartbeat timeout exceeded"),
                10,
            )],
            vec![sample_deployment(
                "deploy-paused-01",
                DeploymentStatus::Paused,
                Some("manual hold"),
                20,
                Some(20),
                None,
            )],
            vec![
                sample_dns_deployment(
                    "dns-deploy-failed-01",
                    DnsDeploymentStatus::Failed,
                    None,
                    30,
                    Some(30),
                    Some(35),
                    2,
                ),
                sample_dns_deployment(
                    "dns-deploy-paused-01",
                    DnsDeploymentStatus::Paused,
                    Some("canary paused"),
                    40,
                    Some(40),
                    None,
                    0,
                ),
                sample_dns_deployment(
                    "dns-deploy-running-01",
                    DnsDeploymentStatus::Running,
                    Some("waiting for canary confirmation"),
                    50,
                    Some(50),
                    None,
                    0,
                ),
            ],
        );

        let kinds = alerts.iter().map(|alert| alert.kind.as_str()).collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                "dns_deployment_failed",
                "node_offline",
                "dns_deployment_attention",
                "dns_deployment_paused",
                "deployment_paused",
            ]
        );

        let failed = alerts
            .iter()
            .find(|alert| alert.kind == "dns_deployment_failed")
            .expect("dns failed alert should exist");
        assert_eq!(failed.resource_type, "dns_deployment");
        assert_eq!(failed.message, "dns revision `dns-v1` failed on 2 target(s)");

        let paused = alerts
            .iter()
            .find(|alert| alert.kind == "dns_deployment_paused")
            .expect("dns paused alert should exist");
        assert_eq!(paused.message, "canary paused");

        let attention = alerts
            .iter()
            .find(|alert| alert.kind == "dns_deployment_attention")
            .expect("dns attention alert should exist");
        assert_eq!(attention.message, "waiting for canary confirmation");
    }

    fn sample_node(
        node_id: &str,
        state: NodeLifecycleState,
        status_reason: Option<&str>,
        last_seen_unix_ms: u64,
    ) -> NodeSummary {
        NodeSummary {
            node_id: node_id.to_string(),
            cluster_id: "cluster-mainland".to_string(),
            advertise_addr: "10.0.0.11:8443".to_string(),
            role: "edge".to_string(),
            state,
            running_version: "v0.1.3-rc.11".to_string(),
            admin_socket_path: "/run/rginx/admin.sock".to_string(),
            last_seen_unix_ms,
            last_snapshot_version: Some(11),
            runtime_revision: Some(21),
            runtime_pid: Some(101),
            listener_count: Some(2),
            active_connections: Some(17),
            status_reason: status_reason.map(ToOwned::to_owned),
        }
    }

    fn sample_deployment(
        deployment_id: &str,
        status: DeploymentStatus,
        status_reason: Option<&str>,
        created_at_unix_ms: u64,
        started_at_unix_ms: Option<u64>,
        finished_at_unix_ms: Option<u64>,
    ) -> DeploymentSummary {
        DeploymentSummary {
            deployment_id: deployment_id.to_string(),
            cluster_id: "cluster-mainland".to_string(),
            revision_id: "rev_local_0001".to_string(),
            revision_version_label: "v0.1.3-rc.11".to_string(),
            status,
            target_nodes: 2,
            healthy_nodes: 1,
            failed_nodes: 0,
            in_flight_nodes: 1,
            parallelism: 1,
            failure_threshold: 1,
            auto_rollback: false,
            created_by: "system".to_string(),
            rollback_of_deployment_id: None,
            rollback_revision_id: None,
            status_reason: status_reason.map(ToOwned::to_owned),
            created_at_unix_ms,
            started_at_unix_ms,
            finished_at_unix_ms,
        }
    }

    fn sample_dns_deployment(
        deployment_id: &str,
        status: DnsDeploymentStatus,
        status_reason: Option<&str>,
        created_at_unix_ms: u64,
        started_at_unix_ms: Option<u64>,
        finished_at_unix_ms: Option<u64>,
        failed_nodes: u32,
    ) -> DnsDeploymentSummary {
        DnsDeploymentSummary {
            deployment_id: deployment_id.to_string(),
            cluster_id: "cluster-mainland".to_string(),
            revision_id: "dns_rev_local_0001".to_string(),
            revision_version_label: "dns-v1".to_string(),
            status,
            target_nodes: 2,
            healthy_nodes: 1,
            failed_nodes,
            active_nodes: 1,
            pending_nodes: 0,
            parallelism: 1,
            failure_threshold: 1,
            auto_rollback: false,
            promotes_cluster_runtime: true,
            created_by: "system".to_string(),
            rollback_of_deployment_id: None,
            rollback_revision_id: None,
            rolled_back_by_deployment_id: None,
            status_reason: status_reason.map(ToOwned::to_owned),
            created_at_unix_ms,
            started_at_unix_ms,
            finished_at_unix_ms,
        }
    }
}
