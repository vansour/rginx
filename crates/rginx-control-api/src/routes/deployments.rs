use axum::{
    Json,
    extract::{Path, State},
};

use rginx_control_types::{
    CreateDeploymentRequest, CreateDeploymentResponse, DeploymentDetail, DeploymentSummary,
};

use crate::auth::{OperatorGuard, ViewerGuard};
use crate::error::{ApiError, ApiResult};
use crate::request_context::RequestContext;
use crate::state::AppState;

pub async fn list_deployments(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<DeploymentSummary>>> {
    let deployments = state
        .services()
        .deployments()
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
) -> ApiResult<Json<DeploymentDetail>> {
    let deployment = state
        .services()
        .deployments()
        .get_deployment_detail(&deployment_id)
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(deployment))
}

pub async fn create_deployment(
    OperatorGuard(actor): OperatorGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
    Json(request): Json<CreateDeploymentRequest>,
) -> ApiResult<Json<CreateDeploymentResponse>> {
    let deployment = state
        .services()
        .deployments()
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
) -> ApiResult<Json<DeploymentDetail>> {
    let deployment = state
        .services()
        .deployments()
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
) -> ApiResult<Json<DeploymentDetail>> {
    let deployment = state
        .services()
        .deployments()
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
