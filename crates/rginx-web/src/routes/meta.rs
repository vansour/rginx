use axum::{Json, extract::State};

use rginx_control_types::ControlPlaneMeta;

use crate::auth::ViewerGuard;
use crate::state::AppState;

pub async fn get_meta(
    ViewerGuard(_actor): ViewerGuard,
    State(state): State<AppState>,
) -> Json<ControlPlaneMeta> {
    Json(state.services().meta().get_meta(state.api_bind_addr().to_string()))
}
