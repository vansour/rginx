use rginx_control_store::{ControlPlaneStore, DashboardSnapshot};
use rginx_control_types::{AlertSeverity, ControlPlaneAlertSummary, DashboardSummary};

use crate::{AlertService, ServiceError, ServiceResult};

#[derive(Debug, Clone)]
pub struct DashboardService {
    store: ControlPlaneStore,
}

impl DashboardService {
    pub fn new(store: ControlPlaneStore) -> Self {
        Self { store }
    }

    pub async fn get_dashboard_summary(&self) -> ServiceResult<DashboardSummary> {
        let snapshot = self
            .store
            .dashboard_repository()
            .load_snapshot()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let alerts = AlertService::new(self.store.clone()).list_current_alerts().await?;
        Ok(Self::build_summary(snapshot, alerts))
    }

    fn build_summary(
        snapshot: DashboardSnapshot,
        alerts: Vec<ControlPlaneAlertSummary>,
    ) -> DashboardSummary {
        let open_alert_count = alerts.len() as u32;
        let critical_alert_count =
            alerts.iter().filter(|alert| alert.severity == AlertSeverity::Critical).count() as u32;
        let warning_alert_count =
            alerts.iter().filter(|alert| alert.severity == AlertSeverity::Warning).count() as u32;

        DashboardSummary {
            total_clusters: snapshot.total_clusters,
            total_nodes: snapshot.total_nodes,
            online_nodes: snapshot.online_nodes,
            draining_nodes: snapshot.draining_nodes,
            offline_nodes: snapshot.offline_nodes,
            drifted_nodes: snapshot.drifted_nodes,
            total_revisions: snapshot.total_revisions,
            active_deployments: snapshot.active_deployments,
            active_dns_deployments: snapshot.active_dns_deployments,
            open_alert_count,
            critical_alert_count,
            warning_alert_count,
            latest_revision: snapshot.latest_revision,
            recent_nodes: snapshot.recent_nodes,
            recent_deployments: snapshot.recent_deployments,
            recent_dns_deployments: snapshot.recent_dns_deployments,
            open_alerts: alerts,
        }
    }
}

#[cfg(test)]
mod tests {
    use rginx_control_store::DashboardSnapshot;
    use rginx_control_types::{
        AlertSeverity, ConfigRevisionSummary, ControlPlaneAlertSummary, DeploymentStatus,
        DeploymentSummary, DnsDeploymentStatus, DnsDeploymentSummary, NodeLifecycleState,
        NodeSummary,
    };

    use super::DashboardService;

    #[test]
    fn dashboard_summary_preserves_snapshot_counts() {
        let summary = DashboardService::build_summary(
            DashboardSnapshot {
                total_clusters: 1,
                total_nodes: 2,
                online_nodes: 1,
                draining_nodes: 1,
                offline_nodes: 0,
                drifted_nodes: 0,
                total_revisions: 3,
                active_deployments: 1,
                active_dns_deployments: 2,
                latest_revision: Some(ConfigRevisionSummary {
                    revision_id: "rev_local_0001".to_string(),
                    cluster_id: "cluster-mainland".to_string(),
                    version_label: "v0.1.3-rc.12".to_string(),
                    created_at_unix_ms: 1_713_513_600_000,
                    summary: "seed control-plane revision".to_string(),
                }),
                recent_nodes: vec![
                    NodeSummary {
                        node_id: "edge-sha-01".to_string(),
                        cluster_id: "cluster-mainland".to_string(),
                        advertise_addr: "10.0.0.11:8443".to_string(),
                        role: "edge".to_string(),
                        state: NodeLifecycleState::Online,
                        running_version: "v0.1.3-rc.12".to_string(),
                        admin_socket_path: "/run/rginx/admin.sock".to_string(),
                        last_seen_unix_ms: 1_713_513_600_000,
                        last_snapshot_version: Some(11),
                        runtime_revision: Some(21),
                        runtime_pid: Some(101),
                        listener_count: Some(2),
                        active_connections: Some(17),
                        status_reason: None,
                    },
                    NodeSummary {
                        node_id: "edge-sz-01".to_string(),
                        cluster_id: "cluster-mainland".to_string(),
                        advertise_addr: "10.0.1.21:8443".to_string(),
                        role: "edge".to_string(),
                        state: NodeLifecycleState::Draining,
                        running_version: "v0.1.3-rc.12".to_string(),
                        admin_socket_path: "/run/rginx/admin.sock".to_string(),
                        last_seen_unix_ms: 1_713_513_600_000,
                        last_snapshot_version: Some(9),
                        runtime_revision: Some(20),
                        runtime_pid: Some(102),
                        listener_count: Some(2),
                        active_connections: Some(4),
                        status_reason: None,
                    },
                ],
                recent_deployments: vec![DeploymentSummary {
                    deployment_id: "deploy_local_0001".to_string(),
                    cluster_id: "cluster-mainland".to_string(),
                    revision_id: "rev_local_0001".to_string(),
                    revision_version_label: "v0.1.3-rc.12".to_string(),
                    status: DeploymentStatus::Running,
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
                    status_reason: None,
                    created_at_unix_ms: 1_713_513_600_000,
                    started_at_unix_ms: Some(1_713_513_600_000),
                    finished_at_unix_ms: None,
                }],
                recent_dns_deployments: vec![DnsDeploymentSummary {
                    deployment_id: "dns_deploy_local_0001".to_string(),
                    cluster_id: "cluster-mainland".to_string(),
                    revision_id: "dns_rev_local_0001".to_string(),
                    revision_version_label: "dns-v1".to_string(),
                    status: DnsDeploymentStatus::Running,
                    target_nodes: 2,
                    healthy_nodes: 1,
                    failed_nodes: 0,
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
                    status_reason: None,
                    created_at_unix_ms: 1_713_513_600_000,
                    started_at_unix_ms: Some(1_713_513_600_000),
                    finished_at_unix_ms: None,
                }],
            },
            vec![ControlPlaneAlertSummary {
                alert_id: "alert_node_offline_edge-sha-01".to_string(),
                cluster_id: Some("cluster-mainland".to_string()),
                severity: AlertSeverity::Critical,
                kind: "node_offline".to_string(),
                title: "Node `edge-sha-01` is offline".to_string(),
                message: "heartbeat timeout exceeded".to_string(),
                resource_type: "node".to_string(),
                resource_id: "edge-sha-01".to_string(),
                observed_at_unix_ms: 1_713_513_600_000,
            }],
        );

        assert_eq!(summary.total_clusters, 1);
        assert_eq!(summary.total_nodes, 2);
        assert_eq!(summary.online_nodes, 1);
        assert_eq!(summary.draining_nodes, 1);
        assert_eq!(summary.offline_nodes, 0);
        assert_eq!(summary.drifted_nodes, 0);
        assert_eq!(summary.total_revisions, 3);
        assert_eq!(summary.active_deployments, 1);
        assert_eq!(summary.active_dns_deployments, 2);
        assert_eq!(summary.open_alert_count, 1);
        assert_eq!(summary.critical_alert_count, 1);
        assert!(summary.latest_revision.is_some());
        assert_eq!(summary.recent_dns_deployments.len(), 1);
    }
}
