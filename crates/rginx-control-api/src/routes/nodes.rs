use axum::{
    Json,
    extract::{Path, State},
};

use rginx_control_types::{NodeDetailResponse, NodeSummary};

use crate::auth::ViewerGuard;
use crate::error::{ApiError, ApiResult};
use crate::request_context::RequestContext;
use crate::state::AppState;

pub async fn list_nodes(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<NodeSummary>>> {
    let nodes = state
        .services()
        .nodes()
        .list_nodes()
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(nodes))
}

pub async fn get_node_detail(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    Path(node_id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<NodeDetailResponse>> {
    let detail = state
        .services()
        .nodes()
        .get_node_detail(&node_id)
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(detail))
}
