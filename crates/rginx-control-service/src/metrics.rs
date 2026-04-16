use rginx_control_store::ControlPlaneStore;
use rginx_control_types::{AlertSeverity, DeploymentStatus, NodeLifecycleState};

use crate::{AlertService, ServiceError, ServiceResult};

#[derive(Debug, Clone)]
pub struct MetricsService {
    store: ControlPlaneStore,
}

impl MetricsService {
    pub fn new(store: ControlPlaneStore) -> Self {
        Self { store }
    }

    pub async fn render_prometheus_metrics(&self) -> ServiceResult<String> {
        let dashboard = self
            .store
            .dashboard_repository()
            .load_snapshot()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
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
        let alerts = AlertService::new(self.store.clone()).list_current_alerts().await?;
        let audit_logs_total = self
            .store
            .audit_repository()
            .count_all()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let node_snapshots_total = self
            .store
            .node_repository()
            .count_snapshots()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;

        let mut body = String::new();
        append_metric_help(
            &mut body,
            "rginx_control_clusters_total",
            "Managed cluster count in the control plane.",
        );
        append_metric(
            &mut body,
            "rginx_control_clusters_total",
            &[],
            dashboard.total_clusters.into(),
        );
        append_metric_help(
            &mut body,
            "rginx_control_revisions_total",
            "Published configuration revision count.",
        );
        append_metric(&mut body, "rginx_control_revisions_total", &[], dashboard.total_revisions);
        append_metric_help(
            &mut body,
            "rginx_control_audit_logs_total",
            "Audit log entries persisted in Postgres.",
        );
        append_metric(&mut body, "rginx_control_audit_logs_total", &[], audit_logs_total);
        append_metric_help(
            &mut body,
            "rginx_control_node_snapshots_total",
            "Node snapshot records persisted in Postgres.",
        );
        append_metric(&mut body, "rginx_control_node_snapshots_total", &[], node_snapshots_total);

        append_metric_help(
            &mut body,
            "rginx_control_nodes_total",
            "Managed nodes by lifecycle state.",
        );
        for state in [
            NodeLifecycleState::Provisioning,
            NodeLifecycleState::Online,
            NodeLifecycleState::Draining,
            NodeLifecycleState::Offline,
            NodeLifecycleState::Drifted,
        ] {
            let count = nodes.iter().filter(|node| node.state == state).count() as u64;
            append_metric(
                &mut body,
                "rginx_control_nodes_total",
                &[("state", state.as_str())],
                count,
            );
        }

        append_metric_help(&mut body, "rginx_control_deployments_total", "Deployments by status.");
        for status in [
            DeploymentStatus::Draft,
            DeploymentStatus::Running,
            DeploymentStatus::Paused,
            DeploymentStatus::Succeeded,
            DeploymentStatus::Failed,
            DeploymentStatus::RolledBack,
        ] {
            let count =
                deployments.iter().filter(|deployment| deployment.status == status).count() as u64;
            append_metric(
                &mut body,
                "rginx_control_deployments_total",
                &[("status", status.as_str())],
                count,
            );
        }

        append_metric_help(
            &mut body,
            "rginx_control_alerts_total",
            "Current derived alerts by severity.",
        );
        for severity in [AlertSeverity::Warning, AlertSeverity::Critical] {
            let count = alerts.iter().filter(|alert| alert.severity == severity).count() as u64;
            append_metric(
                &mut body,
                "rginx_control_alerts_total",
                &[("severity", severity.as_str())],
                count,
            );
        }

        Ok(body)
    }
}

fn append_metric_help(body: &mut String, name: &str, help: &str) {
    body.push_str("# HELP ");
    body.push_str(name);
    body.push(' ');
    body.push_str(help);
    body.push('\n');
    body.push_str("# TYPE ");
    body.push_str(name);
    body.push_str(" gauge\n");
}

fn append_metric(body: &mut String, name: &str, labels: &[(&str, &str)], value: u64) {
    body.push_str(name);
    if !labels.is_empty() {
        body.push('{');
        for (index, (label, label_value)) in labels.iter().enumerate() {
            if index > 0 {
                body.push(',');
            }
            body.push_str(label);
            body.push_str("=\"");
            body.push_str(label_value);
            body.push('"');
        }
        body.push('}');
    }
    body.push(' ');
    body.push_str(&value.to_string());
    body.push('\n');
}
