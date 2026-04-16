use axum::{Json, extract::State};
use rginx_control_service::AuthenticatedNodeAgent;

use rginx_control_types::{
    NodeAgentHeartbeatRequest, NodeAgentRegistrationRequest, NodeAgentTaskAckRequest,
    NodeAgentTaskAckResponse, NodeAgentTaskCompleteRequest, NodeAgentTaskCompleteResponse,
    NodeAgentTaskPollRequest, NodeAgentTaskPollResponse, NodeAgentWriteResponse,
    NodeSnapshotIngestRequest, NodeSnapshotIngestResponse,
};

use crate::auth::{BootstrapAgentGuard, BoundAgentGuard};
use crate::error::{ApiError, ApiResult};
use crate::request_context::RequestContext;
use crate::state::AppState;

pub async fn register(
    _agent_guard: BootstrapAgentGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
    Json(request): Json<NodeAgentRegistrationRequest>,
) -> ApiResult<Json<NodeAgentWriteResponse>> {
    let mut response = state
        .services()
        .nodes()
        .register_agent(
            request,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| {
            ApiError::from(error).with_request_id(request_context.request_id.as_str())
        })?;
    attach_agent_token(
        &state,
        &response.node.node_id,
        &response.node.cluster_id,
        &mut response.agent_token,
        &mut response.agent_token_expires_at_unix_ms,
        request_context.request_id.as_str(),
    )?;
    Ok(Json(response))
}

pub async fn heartbeat(
    BoundAgentGuard(identity): BoundAgentGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
    Json(request): Json<NodeAgentHeartbeatRequest>,
) -> ApiResult<Json<NodeAgentWriteResponse>> {
    let request = bind_agent_report(request, &identity)?;
    let mut response = state
        .services()
        .nodes()
        .record_heartbeat(
            request,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| {
            ApiError::from(error).with_request_id(request_context.request_id.as_str())
        })?;
    attach_agent_token(
        &state,
        &response.node.node_id,
        &response.node.cluster_id,
        &mut response.agent_token,
        &mut response.agent_token_expires_at_unix_ms,
        request_context.request_id.as_str(),
    )?;
    Ok(Json(response))
}

pub async fn ingest_snapshot(
    BoundAgentGuard(identity): BoundAgentGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
    Json(request): Json<NodeSnapshotIngestRequest>,
) -> ApiResult<Json<NodeSnapshotIngestResponse>> {
    let request = bind_snapshot_request(request, &identity)?;
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
        .map_err(|error| {
            ApiError::from(error).with_request_id(request_context.request_id.as_str())
        })?;
    Ok(Json(response))
}

pub async fn poll_tasks(
    BoundAgentGuard(identity): BoundAgentGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
    Json(request): Json<NodeAgentTaskPollRequest>,
) -> ApiResult<Json<NodeAgentTaskPollResponse>> {
    ensure_agent_identity(request.node_id.as_str(), Some(request.cluster_id.as_str()), &identity)?;
    let mut response = state
        .services()
        .deployments()
        .poll_task(&identity.node_id, &identity.cluster_id)
        .await
        .map_err(|error| {
            ApiError::from(error).with_request_id(request_context.request_id.as_str())
        })?;
    attach_agent_token(
        &state,
        &identity.node_id,
        &identity.cluster_id,
        &mut response.agent_token,
        &mut response.agent_token_expires_at_unix_ms,
        request_context.request_id.as_str(),
    )?;
    Ok(Json(response))
}

pub async fn ack_task(
    BoundAgentGuard(identity): BoundAgentGuard,
    request_context: RequestContext,
    axum::extract::Path(task_id): axum::extract::Path<String>,
    State(state): State<AppState>,
    Json(request): Json<NodeAgentTaskAckRequest>,
) -> ApiResult<Json<NodeAgentTaskAckResponse>> {
    ensure_agent_identity(request.node_id.as_str(), None, &identity)?;
    let mut response = state
        .services()
        .deployments()
        .ack_task(
            &task_id,
            &identity.node_id,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| {
            ApiError::from(error).with_request_id(request_context.request_id.as_str())
        })?;
    attach_agent_token(
        &state,
        &identity.node_id,
        &identity.cluster_id,
        &mut response.agent_token,
        &mut response.agent_token_expires_at_unix_ms,
        request_context.request_id.as_str(),
    )?;
    Ok(Json(response))
}

pub async fn complete_task(
    BoundAgentGuard(identity): BoundAgentGuard,
    request_context: RequestContext,
    axum::extract::Path(task_id): axum::extract::Path<String>,
    State(state): State<AppState>,
    Json(request): Json<NodeAgentTaskCompleteRequest>,
) -> ApiResult<Json<NodeAgentTaskCompleteResponse>> {
    ensure_agent_identity(request.node_id.as_str(), None, &identity)?;
    let mut response = state
        .services()
        .deployments()
        .complete_task(
            &task_id,
            &identity.node_id,
            request,
            request_context.idempotency_key,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| {
            ApiError::from(error).with_request_id(request_context.request_id.as_str())
        })?;
    attach_agent_token(
        &state,
        &identity.node_id,
        &identity.cluster_id,
        &mut response.agent_token,
        &mut response.agent_token_expires_at_unix_ms,
        request_context.request_id.as_str(),
    )?;
    Ok(Json(response))
}

fn attach_agent_token(
    state: &AppState,
    node_id: &str,
    cluster_id: &str,
    token_slot: &mut Option<String>,
    expires_at_slot: &mut Option<u64>,
    request_id: &str,
) -> Result<(), ApiError> {
    let grant = state
        .services()
        .auth()
        .mint_node_agent_token(node_id, cluster_id)
        .map_err(|error| ApiError::from(error).with_request_id(request_id.to_string()))?;
    *token_slot = Some(grant.token);
    *expires_at_slot = Some(grant.expires_at_unix_ms);
    Ok(())
}

fn bind_agent_report(
    mut request: NodeAgentRegistrationRequest,
    identity: &AuthenticatedNodeAgent,
) -> Result<NodeAgentRegistrationRequest, ApiError> {
    ensure_agent_identity(request.node_id.as_str(), Some(request.cluster_id.as_str()), identity)?;
    request.node_id = identity.node_id.clone();
    request.cluster_id = identity.cluster_id.clone();
    Ok(request)
}

fn bind_snapshot_request(
    mut request: NodeSnapshotIngestRequest,
    identity: &AuthenticatedNodeAgent,
) -> Result<NodeSnapshotIngestRequest, ApiError> {
    ensure_agent_identity(request.node_id.as_str(), Some(request.cluster_id.as_str()), identity)?;
    request.node_id = identity.node_id.clone();
    request.cluster_id = identity.cluster_id.clone();
    Ok(request)
}

fn ensure_agent_identity(
    request_node_id: &str,
    request_cluster_id: Option<&str>,
    identity: &AuthenticatedNodeAgent,
) -> Result<(), ApiError> {
    if request_node_id.trim().is_empty() {
        return Err(ApiError::unauthorized("missing bound node identity"));
    }
    if request_node_id != identity.node_id {
        return Err(ApiError::forbidden("agent token does not match request node_id"));
    }
    if let Some(request_cluster_id) = request_cluster_id
        && request_cluster_id != identity.cluster_id
    {
        return Err(ApiError::forbidden("agent token does not match request cluster_id"));
    }
    Ok(())
}
