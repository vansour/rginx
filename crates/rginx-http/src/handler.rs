use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

use bytes::Bytes;
use http::StatusCode;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::{Request, Response};
use rginx_core::{ConfigSnapshot, RouteAction};

use crate::proxy::ProxyClients;
use crate::router;

pub(crate) type HttpResponse = Response<Full<Bytes>>;

pub async fn handle(
    request: Request<Incoming>,
    config: Arc<ConfigSnapshot>,
    clients: ProxyClients,
    remote_addr: SocketAddr,
) -> HttpResponse {
    let method = request.method().clone();
    let path = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| request.uri().path().to_string());
    let started = Instant::now();
    let action = router::select_route(&config.routes, request.uri().path())
        .map(|route| route.action.clone());

    let response = match action {
        Some(action) => build_route_response(request, action, clients, remote_addr).await,
        None => {
            text_response(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", "route not found\n")
        }
    };

    let status = response.status();
    tracing::info!(
        method = %method,
        path = %path,
        status = status.as_u16(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "request handled"
    );

    response
}

async fn build_route_response(
    request: Request<Incoming>,
    action: RouteAction,
    clients: ProxyClients,
    remote_addr: SocketAddr,
) -> HttpResponse {
    match &action {
        RouteAction::Static(response) => {
            text_response(response.status, &response.content_type, response.body.clone())
        }
        RouteAction::Proxy(proxy) => {
            crate::proxy::forward_request(clients, request, proxy, remote_addr).await
        }
    }
}

pub(crate) fn text_response(
    status: StatusCode,
    content_type: &str,
    body: impl Into<Bytes>,
) -> HttpResponse {
    Response::builder()
        .status(status)
        .header("content-type", content_type)
        .body(Full::new(body.into()))
        .expect("response builder should not fail for static responses")
}
