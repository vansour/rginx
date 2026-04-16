use rginx_control_store::{AuditLogListFilters, ControlPlaneStore};
use rginx_control_types::{AuditLogEntry, AuditLogSummary};

use crate::{ServiceError, ServiceResult};

#[derive(Debug, Clone)]
pub struct AuditService {
    store: ControlPlaneStore,
}

impl AuditService {
    pub fn new(store: ControlPlaneStore) -> Self {
        Self { store }
    }

    pub async fn list_recent(&self) -> ServiceResult<Vec<AuditLogSummary>> {
        self.store
            .audit_repository()
            .list_recent()
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))
    }

    pub async fn list_entries(
        &self,
        filters: AuditLogListFilters,
    ) -> ServiceResult<Vec<AuditLogEntry>> {
        if filters.limit.is_some_and(|limit| limit <= 0) {
            return Err(ServiceError::BadRequest("limit should be greater than zero".to_string()));
        }
        self.store
            .audit_repository()
            .list_entries(&filters)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))
    }

    pub async fn get_entry(&self, audit_id: &str) -> ServiceResult<AuditLogEntry> {
        self.store
            .audit_repository()
            .load_entry(audit_id)
            .await
            .map_err(|error| ServiceError::Internal(error.to_string()))?
            .ok_or_else(|| ServiceError::NotFound(format!("audit log `{audit_id}` was not found")))
    }
}
