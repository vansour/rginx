use axum::{Json, extract::State};

use rginx_control_types::{AuthLoginRequest, AuthLoginResponse, AuthenticatedActor};

use crate::auth::ViewerGuard;
use crate::error::{ApiError, ApiResult};
use crate::request_context::RequestContext;
use crate::state::AppState;

pub async fn login(
    request_context: RequestContext,
    State(state): State<AppState>,
    Json(request): Json<AuthLoginRequest>,
) -> ApiResult<Json<AuthLoginResponse>> {
    let response = state
        .services()
        .auth()
        .login(
            request,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(response))
}

pub async fn logout(
    ViewerGuard(actor): ViewerGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
) -> ApiResult<Json<serde_json::Value>> {
    state
        .services()
        .auth()
        .logout(
            &actor,
            &request_context.request_id,
            request_context.user_agent,
            request_context.remote_addr,
        )
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(serde_json::json!({ "status": "ok" })))
}

pub async fn get_me(ViewerGuard(actor): ViewerGuard) -> Json<AuthenticatedActor> {
    Json(actor)
}
