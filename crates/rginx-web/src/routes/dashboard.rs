use axum::{Json, extract::State};

use rginx_control_types::DashboardSummary;

use crate::auth::ViewerGuard;
use crate::error::{ApiError, ApiResult};
use crate::request_context::RequestContext;
use crate::state::AppState;

pub async fn get_dashboard(
    ViewerGuard(_actor): ViewerGuard,
    request_context: RequestContext,
    State(state): State<AppState>,
) -> ApiResult<Json<DashboardSummary>> {
    let summary = state
        .services()
        .dashboard()
        .get_dashboard_summary()
        .await
        .map_err(|error| ApiError::from(error).with_request_id(request_context.request_id))?;
    Ok(Json(summary))
}
