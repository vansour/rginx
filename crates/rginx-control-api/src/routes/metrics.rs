use axum::{
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::state::AppState;

pub async fn get_metrics(State(state): State<AppState>) -> Response {
    match state.services().metrics().render_prometheus_metrics().await {
        Ok(body) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
            )],
            body,
        )
            .into_response(),
        Err(error) => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"))],
            format!("metrics collection failed: {error}"),
        )
            .into_response(),
    }
}
