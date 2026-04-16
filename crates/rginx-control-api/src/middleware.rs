use std::time::Instant;

use axum::{
    body::Body,
    http::{HeaderValue, Request},
    middleware::Next,
    response::Response,
};

use crate::request_context::RequestContext;

pub async fn request_context_logging(request: Request<Body>, next: Next) -> Response {
    let started_at = Instant::now();
    let mut request = request;
    let request_context = RequestContext::from_headers(request.headers());
    let method = request.method().clone();
    let path = request.uri().path().to_string();

    request.extensions_mut().insert(request_context.clone());
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        "x-request-id",
        HeaderValue::from_str(&request_context.request_id)
            .unwrap_or_else(|_| HeaderValue::from_static("invalid-request-id")),
    );

    let elapsed_ms = started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    let status = response.status();
    if status.is_server_error() {
        tracing::warn!(
            request_id = %request_context.request_id,
            method = %method,
            path = %path,
            status = status.as_u16(),
            elapsed_ms,
            user_agent = request_context.user_agent.as_deref().unwrap_or("-"),
            remote_addr = request_context.remote_addr.as_deref().unwrap_or("-"),
            "control plane request completed"
        );
    } else {
        tracing::info!(
            request_id = %request_context.request_id,
            method = %method,
            path = %path,
            status = status.as_u16(),
            elapsed_ms,
            user_agent = request_context.user_agent.as_deref().unwrap_or("-"),
            remote_addr = request_context.remote_addr.as_deref().unwrap_or("-"),
            "control plane request completed"
        );
    }

    response
}
