use axum::{Json, extract::State};

use rginx_control_types::{AuthUserSummary, CreateLocalUserRequest, CreateLocalUserResponse};

use crate::auth::SuperAdminGuard;
use crate::error::{ApiError, ApiResult};
use crate::request_context::RequestContext;
use crate::state::AppState;

pub async fn list_users(
    SuperAdminGuard(_actor): SuperAdminGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<AuthUserSummary>>> {
    let users = state
        .services()
        .auth()
        .list_users()
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(users))
}

pub async fn create_user(
    SuperAdminGuard(actor): SuperAdminGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
    Json(request): Json<CreateLocalUserRequest>,
) -> ApiResult<Json<CreateLocalUserResponse>> {
    let response = state
        .services()
        .auth()
        .create_local_user(
            &actor,
            request,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(response))
}
