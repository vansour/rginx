use axum::{
    Json,
    extract::{Path, Query, State},
};
use serde::Deserialize;

use rginx_control_types::{
    CreateDnsDeploymentRequest, CreateDnsDeploymentResponse, CreateDnsDraftRequest,
    DnsDeploymentDetail, DnsDeploymentSummary, DnsDiffResponse, DnsDraftDetail, DnsDraftSummary,
    DnsRevisionDetail, DnsRevisionListItem, DnsRuntimeStatus, DnsSimulationRequest,
    DnsSimulationResponse, PublishDnsDraftRequest, PublishDnsDraftResponse, UpdateDnsDraftRequest,
};

use crate::auth::{OperatorGuard, ViewerGuard};
use crate::error::{ApiError, ApiResult};
use crate::request_context::RequestContext;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct DnsDraftDiffQuery {
    pub target_revision_id: Option<String>,
}

pub async fn list_revisions(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<DnsRevisionListItem>>> {
    let revisions = state
        .services()
        .dns()
        .list_revisions()
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(revisions))
}

pub async fn list_deployments(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<DnsDeploymentSummary>>> {
    let deployments = state
        .services()
        .dns_deployments()
        .list_deployments()
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(deployments))
}

pub async fn get_deployment(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    Path(deployment_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<DnsDeploymentDetail>> {
    let deployment = state
        .services()
        .dns_deployments()
        .get_deployment_detail(&deployment_id)
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(deployment))
}

pub async fn create_deployment(
    OperatorGuard(actor): OperatorGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
    Json(request): Json<CreateDnsDeploymentRequest>,
) -> ApiResult<Json<CreateDnsDeploymentResponse>> {
    let deployment = state
        .services()
        .dns_deployments()
        .create_deployment(
            &actor,
            request,
            request_context.idempotency_key,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(deployment))
}

pub async fn pause_deployment(
    OperatorGuard(actor): OperatorGuard,
    request_context: RequestContext,
    Path(deployment_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<DnsDeploymentDetail>> {
    let deployment = state
        .services()
        .dns_deployments()
        .pause_deployment(
            &actor,
            &deployment_id,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(deployment))
}

pub async fn resume_deployment(
    OperatorGuard(actor): OperatorGuard,
    request_context: RequestContext,
    Path(deployment_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<DnsDeploymentDetail>> {
    let deployment = state
        .services()
        .dns_deployments()
        .resume_deployment(
            &actor,
            &deployment_id,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(deployment))
}

pub async fn rollback_deployment(
    OperatorGuard(actor): OperatorGuard,
    request_context: RequestContext,
    Path(deployment_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<CreateDnsDeploymentResponse>> {
    let deployment = state
        .services()
        .dns_deployments()
        .rollback_deployment(
            &actor,
            &deployment_id,
            request_context.idempotency_key,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(deployment))
}

pub async fn get_revision(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    Path(revision_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<DnsRevisionDetail>> {
    let revision = state
        .services()
        .dns()
        .get_revision(&revision_id)
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(revision))
}

pub async fn list_drafts(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<DnsDraftSummary>>> {
    let drafts = state
        .services()
        .dns()
        .list_drafts()
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(drafts))
}

pub async fn get_draft(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    Path(draft_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<DnsDraftDetail>> {
    let draft = state
        .services()
        .dns()
        .get_draft(&draft_id)
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(draft))
}

pub async fn create_draft(
    OperatorGuard(actor): OperatorGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
    Json(request): Json<CreateDnsDraftRequest>,
) -> ApiResult<Json<DnsDraftDetail>> {
    let draft = state
        .services()
        .dns()
        .create_draft(
            &actor,
            request,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(draft))
}

pub async fn update_draft(
    OperatorGuard(actor): OperatorGuard,
    request_context: RequestContext,
    Path(draft_id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<UpdateDnsDraftRequest>,
) -> ApiResult<Json<DnsDraftDetail>> {
    let draft = state
        .services()
        .dns()
        .update_draft(
            &actor,
            &draft_id,
            request,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(draft))
}

pub async fn validate_draft(
    OperatorGuard(actor): OperatorGuard,
    request_context: RequestContext,
    Path(draft_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<DnsDraftDetail>> {
    let draft = state
        .services()
        .dns()
        .validate_draft(
            &actor,
            &draft_id,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(draft))
}

pub async fn diff_draft(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    Path(draft_id): Path<String>,
    Query(query): Query<DnsDraftDiffQuery>,
    State(state): State<AppState>,
) -> ApiResult<Json<DnsDiffResponse>> {
    let diff = state
        .services()
        .dns()
        .diff_draft(&draft_id, query.target_revision_id.as_deref())
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(diff))
}

pub async fn publish_draft(
    OperatorGuard(actor): OperatorGuard,
    request_context: RequestContext,
    Path(draft_id): Path<String>,
    State(state): State<AppState>,
    Json(request): Json<PublishDnsDraftRequest>,
) -> ApiResult<Json<PublishDnsDraftResponse>> {
    let response = state
        .services()
        .dns()
        .publish_draft(
            &actor,
            &draft_id,
            request,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(response))
}

pub async fn simulate_query(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
    Json(request): Json<DnsSimulationRequest>,
) -> ApiResult<Json<DnsSimulationResponse>> {
    let response = state
        .services()
        .dns()
        .simulate_query(request)
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(response))
}

pub async fn get_runtime_status(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<DnsRuntimeStatus>>> {
    let mut status = state
        .services()
        .dns()
        .runtime_status()
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    if let Some(runtime) = state.dns_runtime() {
        let live = runtime.runtime_status();
        if !live.is_empty() {
            status = live;
        }
    }
    Ok(Json(status))
}
