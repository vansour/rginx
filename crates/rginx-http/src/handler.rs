use std::convert::Infallible;
use std::error::Error as StdError;
use std::net::SocketAddr;
use std::time::Instant;

use bytes::Bytes;
use http::StatusCode;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::BodyExt;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::{Request, Response};
use rginx_core::{ActiveHealthCheck, Route, RouteAction, RouteMatcher};
use serde::Serialize;

use crate::client_ip::{resolve_client_address, ClientAddress};
use crate::metrics::Metrics;
use crate::proxy::PeerStatusSnapshot;
use crate::router;
use crate::state::{ActiveState, SharedState};

pub(crate) type BoxError = Box<dyn StdError + Send + Sync>;
pub(crate) type HttpBody = UnsyncBoxBody<Bytes, BoxError>;
pub(crate) type HttpResponse = Response<HttpBody>;

pub async fn handle(
    request: Request<Incoming>,
    state: SharedState,
    remote_addr: SocketAddr,
) -> HttpResponse {
    let metrics = state.metrics();
    let active = state.snapshot().await;
    let method = request.method().clone();
    let path = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| request.uri().path().to_string());
    let started = Instant::now();
    let client_address =
        resolve_client_address(request.headers(), &active.config.server, remote_addr);
    let selected_route = router::select_route(&active.config.routes, request.uri().path());
    let route_label =
        selected_route.map(route_label).unwrap_or_else(|| "__unmatched__".to_string());
    let response = match selected_route {
        Some(route) => {
            if let Some(response) = authorize_route(route, &client_address) {
                response
            } else if let Some(response) =
                enforce_rate_limit(&state, route, &route_label, &client_address, &metrics)
            {
                response
            } else {
                build_route_response(
                    request,
                    route.action.clone(),
                    active,
                    metrics.clone(),
                    client_address.clone(),
                )
                .await
            }
        }
        None => {
            text_response(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", "route not found\n")
        }
    };

    let status = response.status();
    metrics.observe_http_request(
        &route_label,
        status.as_u16(),
        started.elapsed().as_millis() as u64,
    );
    tracing::info!(
        method = %method,
        path = %path,
        client_ip = %client_address.client_ip,
        client_ip_source = client_address.source.as_str(),
        peer_addr = %client_address.peer_addr,
        status = status.as_u16(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "request handled"
    );

    response
}

fn authorize_route(route: &Route, client_address: &ClientAddress) -> Option<HttpResponse> {
    if route.access_control.allows(client_address.client_ip) {
        return None;
    }

    tracing::warn!(
        client_ip = %client_address.client_ip,
        peer_addr = %client_address.peer_addr,
        route = %route_label(route),
        "request denied by access control"
    );
    Some(forbidden_response())
}

fn enforce_rate_limit(
    state: &SharedState,
    route: &Route,
    route_metric_label: &str,
    client_address: &ClientAddress,
    metrics: &Metrics,
) -> Option<HttpResponse> {
    if state.rate_limiters().check(
        route_metric_label,
        client_address.client_ip,
        route.rate_limit.as_ref(),
    ) {
        return None;
    }

    metrics.record_rate_limited(route_metric_label);
    tracing::warn!(
        client_ip = %client_address.client_ip,
        peer_addr = %client_address.peer_addr,
        route = %route_label(route),
        "request rejected by route rate limit"
    );
    Some(too_many_requests_response())
}

async fn build_route_response(
    request: Request<Incoming>,
    action: RouteAction,
    active: ActiveState,
    metrics: Metrics,
    client_address: ClientAddress,
) -> HttpResponse {
    match &action {
        RouteAction::Static(response) => {
            text_response(response.status, &response.content_type, response.body.clone())
        }
        RouteAction::Proxy(proxy) => {
            let downstream_proto =
                if active.config.server.tls.is_some() { "https" } else { "http" };
            crate::proxy::forward_request(
                active.clients,
                metrics,
                request,
                proxy,
                client_address,
                downstream_proto,
                active.config.server.max_request_body_bytes,
            )
            .await
        }
        RouteAction::Status => status_response(&active),
        RouteAction::Metrics => metrics_response(&metrics),
    }
}

fn status_response(active: &ActiveState) -> HttpResponse {
    let mut upstreams = active
        .config
        .upstreams
        .values()
        .map(|upstream| UpstreamStatusPayload {
            name: upstream.name.clone(),
            request_timeout_ms: upstream.request_timeout.as_millis() as u64,
            active_health_check: upstream.active_health_check.as_ref().map(health_check_payload),
            peers: active.clients.peer_statuses(upstream.as_ref()),
        })
        .collect::<Vec<_>>();
    upstreams.sort_by(|left, right| left.name.cmp(&right.name));

    let payload = StatusPayload {
        revision: active.revision,
        listen: active.config.server.listen_addr.to_string(),
        route_count: active.config.routes.len(),
        upstream_count: active.config.upstreams.len(),
        upstreams,
    };

    json_response(StatusCode::OK, &payload)
}

fn metrics_response(metrics: &Metrics) -> HttpResponse {
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
        .body(full_body(metrics.render_prometheus()))
        .expect("response builder should not fail for metrics responses")
}

fn forbidden_response() -> HttpResponse {
    text_response(StatusCode::FORBIDDEN, "text/plain; charset=utf-8", "forbidden\n")
}

fn too_many_requests_response() -> HttpResponse {
    text_response(StatusCode::TOO_MANY_REQUESTS, "text/plain; charset=utf-8", "too many requests\n")
}

pub(crate) fn text_response(
    status: StatusCode,
    content_type: &str,
    body: impl Into<Bytes>,
) -> HttpResponse {
    Response::builder()
        .status(status)
        .header("content-type", content_type)
        .body(full_body(body))
        .expect("response builder should not fail for static responses")
}

fn json_response(status: StatusCode, payload: &impl Serialize) -> HttpResponse {
    let body = serde_json::to_vec(payload).expect("status payload should serialize");
    Response::builder()
        .status(status)
        .header("content-type", "application/json; charset=utf-8")
        .body(full_body(body))
        .expect("response builder should not fail for JSON responses")
}

pub(crate) fn full_body(body: impl Into<Bytes>) -> HttpBody {
    Full::new(body.into())
        .map_err(|never: Infallible| -> BoxError { match never {} })
        .boxed_unsync()
}

fn health_check_payload(health_check: &ActiveHealthCheck) -> ActiveHealthCheckPayload {
    ActiveHealthCheckPayload {
        path: health_check.path.clone(),
        interval_ms: health_check.interval.as_millis() as u64,
        timeout_ms: health_check.timeout.as_millis() as u64,
        healthy_successes_required: health_check.healthy_successes_required,
    }
}

fn route_label(route: &rginx_core::Route) -> String {
    match &route.matcher {
        RouteMatcher::Exact(path) => format!("exact:{path}"),
        RouteMatcher::Prefix(path) => format!("prefix:{path}"),
    }
}

#[derive(Debug, Serialize)]
struct StatusPayload {
    revision: u64,
    listen: String,
    route_count: usize,
    upstream_count: usize,
    upstreams: Vec<UpstreamStatusPayload>,
}

#[derive(Debug, Serialize)]
struct UpstreamStatusPayload {
    name: String,
    request_timeout_ms: u64,
    active_health_check: Option<ActiveHealthCheckPayload>,
    peers: Vec<PeerStatusSnapshot>,
}

#[derive(Debug, Serialize)]
struct ActiveHealthCheckPayload {
    path: String,
    interval_ms: u64,
    timeout_ms: u64,
    healthy_successes_required: u32,
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    use http::StatusCode;
    use http_body_util::BodyExt;
    use rginx_core::{
        ActiveHealthCheck, ConfigSnapshot, Route, RouteAccessControl, RouteAction, RouteMatcher,
        RouteRateLimit, RuntimeSettings, Server, StaticResponse, Upstream, UpstreamPeer,
        UpstreamTls,
    };
    use serde_json::Value;

    use super::{authorize_route, enforce_rate_limit, metrics_response, status_response};
    use crate::client_ip::{ClientAddress, ClientIpSource};
    use crate::metrics::Metrics;
    use crate::proxy::ProxyClients;
    use crate::state::{ActiveState, SharedState};

    #[tokio::test]
    async fn status_response_includes_revision_and_peer_health() {
        let upstream = Arc::new(Upstream::new(
            "backend".to_string(),
            vec![peer("http://127.0.0.1:9000")],
            UpstreamTls::NativeRoots,
            None,
            Duration::from_secs(30),
            64 * 1024,
            2,
            Duration::from_secs(10),
            Some(ActiveHealthCheck {
                path: "/healthz".to_string(),
                interval: Duration::from_secs(5),
                timeout: Duration::from_secs(2),
                healthy_successes_required: 2,
            }),
        ));
        let config = Arc::new(ConfigSnapshot {
            runtime: RuntimeSettings { shutdown_timeout: Duration::from_secs(1) },
            server: Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                tls: None,
            },
            routes: Vec::new(),
            upstreams: HashMap::from([("backend".to_string(), upstream)]),
        });
        let clients = ProxyClients::from_config(config.as_ref()).expect("clients should build");
        clients.record_active_peer_failure("backend", "http://127.0.0.1:9000");

        let response = status_response(&ActiveState { revision: 3, config, clients });
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").and_then(|value| value.to_str().ok()),
            Some("application/json; charset=utf-8")
        );

        let body =
            response.into_body().collect().await.expect("status body should collect").to_bytes();
        let json: Value =
            serde_json::from_slice(&body).expect("status payload should be valid JSON");

        assert_eq!(json["revision"], 3);
        assert_eq!(json["listen"], "127.0.0.1:8080");
        assert_eq!(json["upstream_count"], 1);
        assert_eq!(json["upstreams"][0]["name"], "backend");
        assert_eq!(json["upstreams"][0]["active_health_check"]["path"], "/healthz");
        assert_eq!(json["upstreams"][0]["peers"][0]["url"], "http://127.0.0.1:9000");
        assert_eq!(json["upstreams"][0]["peers"][0]["healthy"], false);
        assert_eq!(json["upstreams"][0]["peers"][0]["active_unhealthy"], true);
    }

    #[test]
    fn metrics_response_uses_prometheus_content_type() {
        let metrics = Metrics::default();
        metrics.observe_http_request("exact:/metrics", 200, 1);

        let response = metrics_response(&metrics);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").and_then(|value| value.to_str().ok()),
            Some("text/plain; version=0.0.4; charset=utf-8")
        );
    }

    #[test]
    fn authorize_route_rejects_disallowed_remote_addr() {
        let route = Route {
            matcher: RouteMatcher::Exact("/metrics".to_string()),
            action: RouteAction::Static(StaticResponse {
                status: StatusCode::OK,
                content_type: "text/plain; charset=utf-8".to_string(),
                body: "ok\n".to_string(),
            }),
            access_control: RouteAccessControl::new(
                vec!["127.0.0.1/32".parse().unwrap(), "::1/128".parse().unwrap()],
                Vec::new(),
            ),
            rate_limit: None,
        };

        let client_address = ClientAddress {
            peer_addr: "192.0.2.10:4567".parse().unwrap(),
            client_ip: "192.0.2.10".parse().unwrap(),
            forwarded_for: "192.0.2.10".to_string(),
            source: ClientIpSource::SocketPeer,
        };

        let response = authorize_route(&route, &client_address)
            .expect("non-matching address should be rejected");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            response.headers().get("content-type").and_then(|value| value.to_str().ok()),
            Some("text/plain; charset=utf-8")
        );
    }

    #[tokio::test]
    async fn enforce_rate_limit_rejects_requests_after_burst() {
        let config = ConfigSnapshot {
            runtime: RuntimeSettings { shutdown_timeout: Duration::from_secs(1) },
            server: Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                tls: None,
            },
            routes: vec![Route {
                matcher: RouteMatcher::Exact("/api".to_string()),
                action: RouteAction::Static(StaticResponse {
                    status: StatusCode::OK,
                    content_type: "text/plain; charset=utf-8".to_string(),
                    body: "ok\n".to_string(),
                }),
                access_control: RouteAccessControl::default(),
                rate_limit: Some(RouteRateLimit::new(1, 0)),
            }],
            upstreams: HashMap::new(),
        };
        let state = SharedState::from_config(config).expect("shared state should build");
        let metrics = state.metrics();
        let route = state.snapshot().await.config.routes[0].clone();
        let client_address = ClientAddress {
            peer_addr: "192.0.2.10:4567".parse().unwrap(),
            client_ip: "192.0.2.10".parse().unwrap(),
            forwarded_for: "192.0.2.10".to_string(),
            source: ClientIpSource::SocketPeer,
        };

        assert!(
            enforce_rate_limit(&state, &route, "exact:/api", &client_address, &metrics).is_none()
        );

        let response = enforce_rate_limit(&state, &route, "exact:/api", &client_address, &metrics)
            .expect("second request should be rate limited");
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

        let rendered = metrics.render_prometheus();
        assert!(rendered.contains("rginx_http_rate_limited_total{route=\"exact:/api\"} 1"));
    }

    fn peer(url: &str) -> UpstreamPeer {
        let uri: http::Uri = url.parse().expect("peer URL should parse");
        UpstreamPeer {
            url: url.to_string(),
            scheme: uri.scheme_str().expect("peer should have scheme").to_string(),
            authority: uri.authority().expect("peer should have authority").to_string(),
        }
    }
}
