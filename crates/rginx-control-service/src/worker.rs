use std::time::Duration;

use rginx_control_store::ControlPlaneStore;

use crate::{DeploymentService, NodeService, ServiceError, ServiceResult};

#[derive(Debug, Clone)]
pub struct WorkerTickReport {
    pub service_name: String,
    pub known_nodes: usize,
    pub active_deployments: usize,
    pub offline_reconciled_nodes: usize,
    pub dispatched_targets: u32,
    pub finalized_deployments: u32,
    pub rollback_deployments_created: u32,
    pub postgres_endpoint: String,
    pub dragonfly_endpoint: String,
}

#[derive(Debug, Clone)]
pub struct WorkerService {
    store: ControlPlaneStore,
    service_name: String,
    node_offline_threshold: Duration,
}

impl WorkerService {
    pub fn new(
        store: ControlPlaneStore,
        service_name: String,
        node_offline_threshold: Duration,
    ) -> Self {
        Self { store, service_name, node_offline_threshold }
    }

    pub async fn collect_tick_report(&self) -> ServiceResult<WorkerTickReport> {
        let deployment_report =
            DeploymentService::new(self.store.clone()).reconcile_deployments().await?;
        let offline_reconciled_nodes =
            NodeService::new(self.store.clone(), self.node_offline_threshold)
                .reconcile_stale_nodes()
                .await?;
        let context = self
            .store
            .worker_runtime_repository()
            .load_runtime_context()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?;
        let postgres_endpoint = context
            .dependencies
            .iter()
            .find(|dependency| dependency.name == "postgres")
            .map(|dependency| dependency.endpoint.clone())
            .unwrap_or_else(|| self.store.config().postgres_endpoint());
        let dragonfly_endpoint = context
            .dependencies
            .iter()
            .find(|dependency| dependency.name == "dragonfly")
            .map(|dependency| dependency.endpoint.clone())
            .unwrap_or_else(|| self.store.config().dragonfly_endpoint());

        Ok(WorkerTickReport {
            service_name: self.service_name.clone(),
            known_nodes: context.known_nodes,
            active_deployments: context.active_deployments,
            offline_reconciled_nodes,
            dispatched_targets: deployment_report.dispatched_targets,
            finalized_deployments: deployment_report.finalized_deployments,
            rollback_deployments_created: deployment_report.rollback_deployments_created,
            postgres_endpoint,
            dragonfly_endpoint,
        })
    }
}
