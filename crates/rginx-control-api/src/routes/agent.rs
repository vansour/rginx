use axum::{Json, extract::State};

use rginx_control_types::{
    NodeAgentHeartbeatRequest, NodeAgentRegistrationRequest, NodeAgentTaskAckRequest,
    NodeAgentTaskAckResponse, NodeAgentTaskCompleteRequest, NodeAgentTaskCompleteResponse,
    NodeAgentTaskPollRequest, NodeAgentTaskPollResponse, NodeAgentWriteResponse,
    NodeSnapshotIngestRequest, NodeSnapshotIngestResponse,
};

use crate::auth::AgentGuard;
use crate::error::{ApiError, ApiResult};
use crate::request_context::RequestContext;
use crate::state::AppState;

pub async fn register(
    _agent_guard: AgentGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
    Json(request): Json<NodeAgentRegistrationRequest>,
) -> ApiResult<Json<NodeAgentWriteResponse>> {
    let response = state
        .services()
        .nodes()
        .register_agent(
            request,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(response))
}

pub async fn heartbeat(
    _agent_guard: AgentGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
    Json(request): Json<NodeAgentHeartbeatRequest>,
) -> ApiResult<Json<NodeAgentWriteResponse>> {
    let response = state
        .services()
        .nodes()
        .record_heartbeat(
            request,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(response))
}

pub async fn ingest_snapshot(
    _agent_guard: AgentGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
    Json(request): Json<NodeSnapshotIngestRequest>,
) -> ApiResult<Json<NodeSnapshotIngestResponse>> {
    let response = state
        .services()
        .nodes()
        .ingest_snapshot(
            request,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(response))
}

pub async fn poll_tasks(
    _agent_guard: AgentGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
    Json(request): Json<NodeAgentTaskPollRequest>,
) -> ApiResult<Json<NodeAgentTaskPollResponse>> {
    let response = state
        .services()
        .deployments()
        .poll_task(request)
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(response))
}

pub async fn ack_task(
    _agent_guard: AgentGuard,
    request_context: RequestContext,
    axum::extract::Path(task_id): axum::extract::Path<String>,
    State(state): State<AppState>,
    Json(request): Json<NodeAgentTaskAckRequest>,
) -> ApiResult<Json<NodeAgentTaskAckResponse>> {
    let response = state
        .services()
        .deployments()
        .ack_task(
            &task_id,
            request,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(response))
}

pub async fn complete_task(
    _agent_guard: AgentGuard,
    request_context: RequestContext,
    axum::extract::Path(task_id): axum::extract::Path<String>,
    State(state): State<AppState>,
    Json(request): Json<NodeAgentTaskCompleteRequest>,
) -> ApiResult<Json<NodeAgentTaskCompleteResponse>> {
    let response = state
        .services()
        .deployments()
        .complete_task(
            &task_id,
            request,
            request_context.idempotency_key,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(response))
}
