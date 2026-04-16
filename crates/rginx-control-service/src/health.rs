use rginx_control_store::ControlPlaneStore;
use rginx_control_types::ServiceHealth;

use crate::{ServiceError, ServiceResult};

#[derive(Debug, Clone)]
pub struct HealthService {
    store: ControlPlaneStore,
    service_name: String,
}

impl HealthService {
    pub fn new(store: ControlPlaneStore, service_name: String) -> Self {
        Self { store, service_name }
    }

    pub async fn get_service_health(&self) -> ServiceResult<ServiceHealth> {
        self.store
            .dependency_repository()
            .ensure_postgres_ready()
            .await
            .map_err(|error| ServiceError::DependencyUnavailable(error.to_string()))?;

        Ok(ServiceHealth { service: self.service_name.clone(), status: "ok".to_string() })
    }
}
