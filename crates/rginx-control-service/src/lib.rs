mod alerts;
mod audit;
mod auth;
mod config;
mod dashboard;
mod deployments;
mod error;
mod health;
mod meta;
mod metrics;
mod nodes;
mod revisions;
mod worker;

pub use alerts::AlertService;
pub use audit::AuditService;
pub use auth::AuthService;
pub use config::{ControlPlaneAuthConfig, ControlPlaneServiceConfig};
pub use dashboard::DashboardService;
pub use deployments::{DeploymentReconcileReport, DeploymentService};
pub use error::{ServiceError, ServiceResult};
pub use health::HealthService;
pub use meta::MetaService;
pub use metrics::MetricsService;
pub use nodes::NodeService;
pub use revisions::RevisionService;
pub use worker::{WorkerService, WorkerTickReport};

use rginx_control_store::ControlPlaneStore;

#[derive(Debug, Clone)]
pub struct ControlPlaneServices {
    auth: AuthService,
    audit: AuditService,
    alerts: AlertService,
    meta: MetaService,
    health: HealthService,
    dashboard: DashboardService,
    metrics: MetricsService,
    deployments: DeploymentService,
    revisions: RevisionService,
    nodes: NodeService,
    worker: WorkerService,
}

impl ControlPlaneServices {
    pub fn new(store: ControlPlaneStore, config: ControlPlaneServiceConfig) -> Self {
        Self {
            auth: AuthService::new(store.clone(), config.auth.clone()),
            audit: AuditService::new(store.clone()),
            alerts: AlertService::new(store.clone()),
            meta: MetaService::new(config.clone()),
            health: HealthService::new(store.clone(), config.service_name.clone()),
            dashboard: DashboardService::new(store.clone()),
            metrics: MetricsService::new(store.clone()),
            deployments: DeploymentService::new(store.clone()),
            revisions: RevisionService::new(store.clone()),
            nodes: NodeService::new(store.clone(), config.node_offline_threshold),
            worker: WorkerService::new(store, config.service_name, config.node_offline_threshold),
        }
    }

    pub fn auth(&self) -> &AuthService {
        &self.auth
    }

    pub fn audit(&self) -> &AuditService {
        &self.audit
    }

    pub fn alerts(&self) -> &AlertService {
        &self.alerts
    }

    pub fn meta(&self) -> &MetaService {
        &self.meta
    }

    pub fn health(&self) -> &HealthService {
        &self.health
    }

    pub fn dashboard(&self) -> &DashboardService {
        &self.dashboard
    }

    pub fn metrics(&self) -> &MetricsService {
        &self.metrics
    }

    pub fn nodes(&self) -> &NodeService {
        &self.nodes
    }

    pub fn deployments(&self) -> &DeploymentService {
        &self.deployments
    }

    pub fn revisions(&self) -> &RevisionService {
        &self.revisions
    }

    pub fn worker(&self) -> &WorkerService {
        &self.worker
    }
}
