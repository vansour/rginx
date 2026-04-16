use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;

use rginx_control_store::AuditLogListFilters;
use rginx_control_types::AuditLogEntry;

use crate::auth::OperatorGuard;
use crate::error::{ApiError, ApiResult};
use crate::request_context::RequestContext;
use crate::state::AppState;

#[derive(Debug, Deserialize, Default)]
pub struct AuditLogsQuery {
    pub cluster_id: Option<String>,
    pub actor_id: Option<String>,
    pub action: Option<String>,
    pub resource_type: Option<String>,
    pub resource_id: Option<String>,
    pub result: Option<String>,
    pub limit: Option<i64>,
}

pub async fn list_audit_logs(
    OperatorGuard(_actor): OperatorGuard,
    request_context: RequestContext,
    Query(query): Query<AuditLogsQuery>,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<AuditLogEntry>>> {
    let audit_logs = state
        .services()
        .audit()
        .list_entries(AuditLogListFilters {
            cluster_id: query.cluster_id,
            actor_id: query.actor_id,
            action: query.action,
            resource_type: query.resource_type,
            resource_id: query.resource_id,
            result: query.result,
            limit: query.limit,
        })
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(audit_logs))
}

pub async fn get_audit_log(
    OperatorGuard(_actor): OperatorGuard,
    request_context: RequestContext,
    Path(audit_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<AuditLogEntry>> {
    let audit_log = state
        .services()
        .audit()
        .get_entry(&audit_id)
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(audit_log))
}
