use axum::{Json, extract::State};

use rginx_control_types::ControlPlaneAlertSummary;

use crate::auth::ViewerGuard;
use crate::error::{ApiError, ApiResult};
use crate::request_context::RequestContext;
use crate::state::AppState;

pub async fn list_alerts(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<ControlPlaneAlertSummary>>> {
    let alerts = state
        .services()
        .alerts()
        .list_current_alerts()
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(alerts))
}
