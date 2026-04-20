use axum::{Json, extract::State};

use rginx_control_types::ServiceHealth;

use crate::error::{ApiError, ApiResult};
use crate::request_context::RequestContext;
use crate::state::AppState;

pub async fn get_health(
    request_context: RequestContext,
    State(state): State<AppState>,
) -> ApiResult<Json<ServiceHealth>> {
    let health = state
        .services()
        .health()
        .get_service_health()
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(health))
}
