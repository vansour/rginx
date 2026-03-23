use std::convert::Infallible;
use std::error::Error as StdError;
use std::net::SocketAddr;
use std::time::Instant;

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header::HOST};
use http_body_util::BodyExt;
use http_body_util::Full;
use http_body_util::combinators::UnsyncBoxBody;
use hyper::body::Incoming;
use hyper::{Request, Response};
use rginx_core::{ActiveHealthCheck, ConfigSnapshot, Route, RouteAction, VirtualHost};
use serde::Serialize;

use crate::client_ip::{ClientAddress, resolve_client_address};
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
    let mut request = request;
    let metrics = state.metrics();
    let active = state.snapshot().await;
    let method = request.method().clone();
    let host = request_host(request.headers(), request.uri()).to_string();
    let request_id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| state.next_request_id());
    let request_id_header =
        HeaderValue::from_str(&request_id).expect("generated request ids should be valid headers");
    request.headers_mut().insert("x-request-id", request_id_header.clone());
    let path = request
        .uri()
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| request.uri().path().to_string());
    let started = Instant::now();
    let client_address =
        resolve_client_address(request.headers(), &active.config.server, remote_addr);
    let (selected_vhost_id, selected_route) = {
        let selected_vhost =
            select_vhost_for_request(active.config.as_ref(), request.headers(), request.uri());
        (
            selected_vhost.id.clone(),
            router::select_route_in_vhost(selected_vhost, request.uri().path()).cloned(),
        )
    };
    let route_id = selected_route
        .as_ref()
        .map(|route| route.id.clone())
        .unwrap_or_else(|| "__unmatched__".to_string());
    let mut response = match selected_route {
        Some(route) => {
            if let Some(response) = authorize_route(&route, &client_address) {
                response
            } else if let Some(response) =
                enforce_rate_limit(&state, &route, &route_id, &client_address, &metrics)
            {
                response
            } else {
                build_route_response(
                    request,
                    state.clone(),
                    route.action,
                    active,
                    metrics.clone(),
                    client_address.clone(),
                    &request_id,
                )
                .await
            }
        }
        None => {
            text_response(StatusCode::NOT_FOUND, "text/plain; charset=utf-8", "route not found\n")
        }
    };
    if method == Method::HEAD {
        response = strip_response_body(response);
    }
    response.headers_mut().insert("x-request-id", request_id_header);

    let status = response.status();
    metrics.observe_http_request(&route_id, status.as_u16(), started.elapsed().as_millis() as u64);
    tracing::info!(
        request_id = %request_id,
        method = %method,
        host = %host,
        path = %path,
        client_ip = %client_address.client_ip,
        client_ip_source = client_address.source.as_str(),
        peer_addr = %client_address.peer_addr,
        vhost = %selected_vhost_id,
        route = %route_id,
        status = status.as_u16(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "http access"
    );

    response
}

fn request_host<'a>(headers: &'a HeaderMap, uri: &'a Uri) -> &'a str {
    headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| uri.authority().map(|authority| authority.as_str()))
        .unwrap_or_default()
}

fn select_vhost_for_request<'a>(
    config: &'a ConfigSnapshot,
    headers: &HeaderMap,
    uri: &Uri,
) -> &'a VirtualHost {
    let host = request_host(headers, uri).to_string();
    router::select_vhost(&config.vhosts, &config.default_vhost, &host)
}

#[cfg(test)]
fn select_route_for_request<'a>(
    config: &'a ConfigSnapshot,
    headers: &HeaderMap,
    uri: &Uri,
) -> Option<&'a Route> {
    let vhost = select_vhost_for_request(config, headers, uri);
    router::select_route_in_vhost(vhost, uri.path())
}

fn authorize_route(route: &Route, client_address: &ClientAddress) -> Option<HttpResponse> {
    if route.access_control.allows(client_address.client_ip) {
        return None;
    }

    tracing::warn!(
        client_ip = %client_address.client_ip,
        peer_addr = %client_address.peer_addr,
        route = %route.id,
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
        route = %route.id,
        "request rejected by route rate limit"
    );
    Some(too_many_requests_response())
}

async fn build_route_response(
    request: Request<Incoming>,
    state: SharedState,
    action: RouteAction,
    active: ActiveState,
    metrics: Metrics,
    client_address: ClientAddress,
    request_id: &str,
) -> HttpResponse {
    match &action {
        RouteAction::Static(response) => {
            text_response(response.status, &response.content_type, response.body.clone())
        }
        RouteAction::Proxy(proxy) => {
            let downstream_proto =
                if active.config.server.tls.is_some() { "https" } else { "http" };
            crate::proxy::forward_request(
                state,
                active.clients,
                request,
                proxy,
                client_address,
                downstream_proto,
                active.config.server.max_request_body_bytes,
                request_id,
            )
            .await
        }
        RouteAction::Status => status_response(&active),
        RouteAction::Metrics => metrics_response(&metrics),
        RouteAction::File(target) => crate::file::serve_file(request, target).await,
        RouteAction::Return(action) => {
            let body = action.body.clone().unwrap_or_else(|| {
                format!("{}\n", action.status.canonical_reason().unwrap_or("Redirect"))
            });
            let content_length = body.len();

            let mut builder = Response::builder()
                .status(action.status)
                .header("content-type", "text/plain; charset=utf-8")
                .header("content-length", content_length.to_string());

            if !action.location.is_empty() {
                builder = builder.header("location", &action.location);
            }

            builder.body(full_body(body)).expect("return response builder should not fail")
        }
    }
}

fn status_response(active: &ActiveState) -> HttpResponse {
    let mut upstreams = active
        .config
        .upstreams
        .values()
        .map(|upstream| UpstreamStatusPayload {
            name: upstream.name.clone(),
            protocol: upstream.protocol.as_str(),
            load_balance: upstream.load_balance.as_str(),
            request_timeout_ms: upstream.request_timeout.as_millis() as u64,
            read_timeout_ms: upstream.request_timeout.as_millis() as u64,
            connect_timeout_ms: upstream.connect_timeout.as_millis() as u64,
            write_timeout_ms: upstream.write_timeout.as_millis() as u64,
            idle_timeout_ms: upstream.idle_timeout.as_millis() as u64,
            pool_idle_timeout_ms: upstream
                .pool_idle_timeout
                .map(|timeout| timeout.as_millis() as u64),
            pool_max_idle_per_host: upstream.pool_max_idle_per_host,
            tcp_keepalive_ms: upstream.tcp_keepalive.map(|timeout| timeout.as_millis() as u64),
            tcp_nodelay: upstream.tcp_nodelay,
            http2_keep_alive_interval_ms: upstream
                .http2_keep_alive_interval
                .map(|timeout| timeout.as_millis() as u64),
            http2_keep_alive_timeout_ms: upstream.http2_keep_alive_timeout.as_millis() as u64,
            http2_keep_alive_while_idle: upstream.http2_keep_alive_while_idle,
            active_health_check: upstream.active_health_check.as_ref().map(health_check_payload),
            peers: active.clients.peer_statuses(upstream.as_ref()),
        })
        .collect::<Vec<_>>();
    upstreams.sort_by(|left, right| left.name.cmp(&right.name));

    let payload = StatusPayload {
        revision: active.revision,
        listen: active.config.server.listen_addr.to_string(),
        vhost_count: active.config.total_vhost_count(),
        route_count: active.config.total_route_count(),
        upstream_count: active.config.upstreams.len(),
        upstreams,
    };

    json_response(StatusCode::OK, &payload)
}

fn metrics_response(metrics: &Metrics) -> HttpResponse {
    let body = metrics.render_prometheus();
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/plain; version=0.0.4; charset=utf-8")
        .header("content-length", body.len().to_string())
        .body(full_body(body))
        .expect("response builder should not fail for metrics responses")
}

fn forbidden_response() -> HttpResponse {
    text_response(StatusCode::FORBIDDEN, "text/plain; charset=utf-8", "forbidden\n")
}

fn too_many_requests_response() -> HttpResponse {
    text_response(
        StatusCode::TOO_MANY_REQUESTS,
        "text/plain; charset=utf-8",
        "hold your horses! too many requests\n",
    )
}

pub(crate) fn text_response(
    status: StatusCode,
    content_type: &str,
    body: impl Into<Bytes>,
) -> HttpResponse {
    let body = body.into();
    Response::builder()
        .status(status)
        .header("content-type", content_type)
        .header("content-length", body.len().to_string())
        .body(full_body(body))
        .expect("response builder should not fail for static responses")
}

fn json_response(status: StatusCode, payload: &impl Serialize) -> HttpResponse {
    let body = serde_json::to_vec(payload).expect("status payload should serialize");
    Response::builder()
        .status(status)
        .header("content-type", "application/json; charset=utf-8")
        .header("content-length", body.len().to_string())
        .body(full_body(body))
        .expect("response builder should not fail for JSON responses")
}

pub(crate) fn full_body(body: impl Into<Bytes>) -> HttpBody {
    Full::new(body.into())
        .map_err(|never: Infallible| -> BoxError { match never {} })
        .boxed_unsync()
}

fn strip_response_body(response: HttpResponse) -> HttpResponse {
    let (parts, _body) = response.into_parts();
    Response::from_parts(parts, full_body(Bytes::new()))
}

fn health_check_payload(health_check: &ActiveHealthCheck) -> ActiveHealthCheckPayload {
    ActiveHealthCheckPayload {
        path: health_check.path.clone(),
        interval_ms: health_check.interval.as_millis() as u64,
        timeout_ms: health_check.timeout.as_millis() as u64,
        healthy_successes_required: health_check.healthy_successes_required,
    }
}

#[derive(Debug, Serialize)]
struct StatusPayload {
    revision: u64,
    listen: String,
    vhost_count: usize,
    route_count: usize,
    upstream_count: usize,
    upstreams: Vec<UpstreamStatusPayload>,
}

#[derive(Debug, Serialize)]
struct UpstreamStatusPayload {
    name: String,
    protocol: &'static str,
    load_balance: &'static str,
    request_timeout_ms: u64,
    read_timeout_ms: u64,
    connect_timeout_ms: u64,
    write_timeout_ms: u64,
    idle_timeout_ms: u64,
    pool_idle_timeout_ms: Option<u64>,
    pool_max_idle_per_host: usize,
    tcp_keepalive_ms: Option<u64>,
    tcp_nodelay: bool,
    http2_keep_alive_interval_ms: Option<u64>,
    http2_keep_alive_timeout_ms: u64,
    http2_keep_alive_while_idle: bool,
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

    use http::{HeaderMap, HeaderValue, StatusCode, header::HOST};
    use http_body_util::BodyExt;
    use rginx_core::{
        ActiveHealthCheck, ConfigSnapshot, Route, RouteAccessControl, RouteAction, RouteMatcher,
        RouteRateLimit, RuntimeSettings, Server, StaticResponse, Upstream, UpstreamLoadBalance,
        UpstreamPeer, UpstreamProtocol, UpstreamSettings, UpstreamTls, VirtualHost,
    };
    use serde_json::Value;

    use super::{
        authorize_route, enforce_rate_limit, metrics_response, select_route_for_request,
        status_response,
    };
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
            UpstreamSettings {
                load_balance: UpstreamLoadBalance::IpHash,
                connect_timeout: Duration::from_secs(3),
                request_timeout: Duration::from_secs(4),
                write_timeout: Duration::from_secs(5),
                idle_timeout: Duration::from_secs(6),
                pool_idle_timeout: Some(Duration::from_secs(7)),
                pool_max_idle_per_host: 8,
                tcp_keepalive: Some(Duration::from_secs(9)),
                tcp_nodelay: true,
                http2_keep_alive_interval: Some(Duration::from_secs(10)),
                http2_keep_alive_timeout: Duration::from_secs(11),
                http2_keep_alive_while_idle: true,
                active_health_check: Some(ActiveHealthCheck {
                    path: "/healthz".to_string(),
                    interval: Duration::from_secs(5),
                    timeout: Duration::from_secs(2),
                    healthy_successes_required: 2,
                }),
                ..upstream_settings()
            },
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
            default_vhost: VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
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
        assert_eq!(json["vhost_count"], 1);
        assert_eq!(json["route_count"], 0);
        assert_eq!(json["upstream_count"], 1);
        assert_eq!(json["upstreams"][0]["name"], "backend");
        assert_eq!(json["upstreams"][0]["protocol"], "auto");
        assert_eq!(json["upstreams"][0]["load_balance"], "ip_hash");
        assert_eq!(json["upstreams"][0]["request_timeout_ms"], 4_000);
        assert_eq!(json["upstreams"][0]["read_timeout_ms"], 4_000);
        assert_eq!(json["upstreams"][0]["connect_timeout_ms"], 3_000);
        assert_eq!(json["upstreams"][0]["write_timeout_ms"], 5_000);
        assert_eq!(json["upstreams"][0]["idle_timeout_ms"], 6_000);
        assert_eq!(json["upstreams"][0]["pool_idle_timeout_ms"], 7_000);
        assert_eq!(json["upstreams"][0]["pool_max_idle_per_host"], 8);
        assert_eq!(json["upstreams"][0]["tcp_keepalive_ms"], 9_000);
        assert_eq!(json["upstreams"][0]["tcp_nodelay"], true);
        assert_eq!(json["upstreams"][0]["http2_keep_alive_interval_ms"], 10_000);
        assert_eq!(json["upstreams"][0]["http2_keep_alive_timeout_ms"], 11_000);
        assert_eq!(json["upstreams"][0]["http2_keep_alive_while_idle"], true);
        assert_eq!(json["upstreams"][0]["active_health_check"]["path"], "/healthz");
        assert_eq!(json["upstreams"][0]["peers"][0]["url"], "http://127.0.0.1:9000");
        assert_eq!(json["upstreams"][0]["peers"][0]["weight"], 1);
        assert_eq!(json["upstreams"][0]["peers"][0]["backup"], false);
        assert_eq!(json["upstreams"][0]["peers"][0]["active_requests"], 0);
        assert_eq!(json["upstreams"][0]["peers"][0]["healthy"], false);
        assert_eq!(json["upstreams"][0]["peers"][0]["active_unhealthy"], true);
    }

    #[tokio::test]
    async fn status_response_counts_routes_across_all_vhosts() {
        let config = Arc::new(test_config(
            test_vhost(
                "server",
                vec!["default.example.com"],
                vec![test_route("server/routes[0]|exact:/", RouteMatcher::Exact("/".to_string()))],
            ),
            vec![test_vhost(
                "servers[0]",
                vec!["api.example.com"],
                vec![
                    test_route(
                        "servers[0]/routes[0]|exact:/users",
                        RouteMatcher::Exact("/users".to_string()),
                    ),
                    test_route(
                        "servers[0]/routes[1]|exact:/status",
                        RouteMatcher::Exact("/status".to_string()),
                    ),
                ],
            )],
        ));
        let clients = ProxyClients::from_config(config.as_ref()).expect("clients should build");

        let response = status_response(&ActiveState { revision: 7, config, clients });
        let body =
            response.into_body().collect().await.expect("status body should collect").to_bytes();
        let json: Value =
            serde_json::from_slice(&body).expect("status payload should be valid JSON");

        assert_eq!(json["revision"], 7);
        assert_eq!(json["vhost_count"], 2);
        assert_eq!(json["route_count"], 3);
        assert_eq!(json["upstream_count"], 0);
    }

    #[test]
    fn metrics_response_uses_prometheus_content_type() {
        let metrics = Metrics::default();
        metrics.observe_http_request("server/routes[0]|exact:/metrics", 200, 1);

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
            id: "server/routes[0]|exact:/metrics".to_string(),
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
            default_vhost: VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: vec![Route {
                    id: "server/routes[0]|exact:/api".to_string(),
                    matcher: RouteMatcher::Exact("/api".to_string()),
                    action: RouteAction::Static(StaticResponse {
                        status: StatusCode::OK,
                        content_type: "text/plain; charset=utf-8".to_string(),
                        body: "ok\n".to_string(),
                    }),
                    access_control: RouteAccessControl::default(),
                    rate_limit: Some(RouteRateLimit::new(1, 0)),
                }],
                tls: None,
            },
            vhosts: Vec::new(),
            upstreams: HashMap::new(),
        };
        let state = SharedState::from_config(config).expect("shared state should build");
        let metrics = state.metrics();
        let route = state.snapshot().await.config.default_vhost.routes[0].clone();
        let client_address = ClientAddress {
            peer_addr: "192.0.2.10:4567".parse().unwrap(),
            client_ip: "192.0.2.10".parse().unwrap(),
            forwarded_for: "192.0.2.10".to_string(),
            source: ClientIpSource::SocketPeer,
        };

        assert!(enforce_rate_limit(&state, &route, &route.id, &client_address, &metrics).is_none());

        let response = enforce_rate_limit(&state, &route, &route.id, &client_address, &metrics)
            .expect("second request should be rate limited");
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);

        let rendered = metrics.render_prometheus();
        assert!(
            rendered
                .contains("rginx_http_rate_limited_total{route=\"server/routes[0]|exact:/api\"} 1")
        );
    }

    #[test]
    fn select_route_for_request_uses_host_specific_vhost_routes() {
        let config = test_config(
            test_vhost(
                "server",
                Vec::new(),
                vec![test_route("server/routes[0]|exact:/", RouteMatcher::Exact("/".to_string()))],
            ),
            vec![test_vhost(
                "servers[0]",
                vec!["api.example.com"],
                vec![test_route(
                    "servers[0]/routes[0]|exact:/users",
                    RouteMatcher::Exact("/users".to_string()),
                )],
            )],
        );

        let route = select_route_for_request(
            &config,
            &host_headers("api.example.com"),
            &request_uri("/users"),
        )
        .expect("api.example.com should match vhost route");
        assert_eq!(route.id, "servers[0]/routes[0]|exact:/users");
    }

    #[test]
    fn select_route_for_request_falls_back_to_default_vhost_for_unknown_host() {
        let config = test_config(
            test_vhost(
                "server",
                Vec::new(),
                vec![test_route("server/routes[0]|exact:/", RouteMatcher::Exact("/".to_string()))],
            ),
            vec![test_vhost(
                "servers[0]",
                vec!["api.example.com"],
                vec![test_route(
                    "servers[0]/routes[0]|exact:/",
                    RouteMatcher::Exact("/".to_string()),
                )],
            )],
        );

        let route = select_route_for_request(
            &config,
            &host_headers("unknown.example.com"),
            &request_uri("/"),
        )
        .expect("unknown host should use default vhost");
        assert_eq!(route.id, "server/routes[0]|exact:/");
    }

    #[test]
    fn select_route_for_request_supports_wildcard_hosts() {
        let config = test_config(
            test_vhost("server", Vec::new(), Vec::new()),
            vec![test_vhost(
                "servers[0]",
                vec!["*.internal.example.com"],
                vec![test_route(
                    "servers[0]/routes[0]|exact:/healthz",
                    RouteMatcher::Exact("/healthz".to_string()),
                )],
            )],
        );

        let route = select_route_for_request(
            &config,
            &host_headers("app.internal.example.com:8443"),
            &request_uri("/healthz"),
        )
        .expect("wildcard host should match vhost route");
        assert_eq!(route.id, "servers[0]/routes[0]|exact:/healthz");
    }

    #[test]
    fn select_route_for_request_uses_uri_authority_when_host_header_is_absent() {
        let config = test_config(
            test_vhost("server", Vec::new(), Vec::new()),
            vec![test_vhost(
                "servers[0]",
                vec!["api.example.com"],
                vec![test_route(
                    "servers[0]/routes[0]|exact:/users",
                    RouteMatcher::Exact("/users".to_string()),
                )],
            )],
        );

        let route = select_route_for_request(
            &config,
            &HeaderMap::new(),
            &"https://api.example.com/users".parse().unwrap(),
        )
        .expect("request URI authority should be used when host header is absent");
        assert_eq!(route.id, "servers[0]/routes[0]|exact:/users");
    }

    #[test]
    fn select_route_for_request_does_not_fall_back_when_matched_vhost_has_no_route() {
        let config = test_config(
            test_vhost(
                "server",
                Vec::new(),
                vec![test_route(
                    "server/routes[0]|exact:/users",
                    RouteMatcher::Exact("/users".to_string()),
                )],
            ),
            vec![test_vhost(
                "servers[0]",
                vec!["api.example.com"],
                vec![test_route(
                    "servers[0]/routes[0]|exact:/status",
                    RouteMatcher::Exact("/status".to_string()),
                )],
            )],
        );

        let route = select_route_for_request(
            &config,
            &host_headers("api.example.com"),
            &request_uri("/users"),
        );
        assert!(route.is_none(), "matched vhost without matching path should return 404");
    }

    fn peer(url: &str) -> UpstreamPeer {
        let uri: http::Uri = url.parse().expect("peer URL should parse");
        UpstreamPeer {
            url: url.to_string(),
            scheme: uri.scheme_str().expect("peer should have scheme").to_string(),
            authority: uri.authority().expect("peer should have authority").to_string(),
            weight: 1,
            backup: false,
        }
    }

    fn upstream_settings() -> UpstreamSettings {
        UpstreamSettings {
            protocol: UpstreamProtocol::Auto,
            load_balance: UpstreamLoadBalance::RoundRobin,
            server_name_override: None,
            request_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(30),
            pool_idle_timeout: Some(Duration::from_secs(90)),
            pool_max_idle_per_host: usize::MAX,
            tcp_keepalive: None,
            tcp_nodelay: false,
            http2_keep_alive_interval: None,
            http2_keep_alive_timeout: Duration::from_secs(20),
            http2_keep_alive_while_idle: false,
            max_replayable_request_body_bytes: 64 * 1024,
            unhealthy_after_failures: 2,
            unhealthy_cooldown: Duration::from_secs(10),
            active_health_check: None,
        }
    }

    fn test_config(default_vhost: VirtualHost, vhosts: Vec<VirtualHost>) -> ConfigSnapshot {
        ConfigSnapshot {
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
            default_vhost,
            vhosts,
            upstreams: HashMap::new(),
        }
    }

    fn test_vhost(id: &str, server_names: Vec<&str>, routes: Vec<Route>) -> VirtualHost {
        VirtualHost {
            id: id.to_string(),
            server_names: server_names.into_iter().map(str::to_string).collect(),
            routes,
            tls: None,
        }
    }

    fn test_route(id: &str, matcher: RouteMatcher) -> Route {
        Route {
            id: id.to_string(),
            matcher,
            action: RouteAction::Static(StaticResponse {
                status: StatusCode::OK,
                content_type: "text/plain; charset=utf-8".to_string(),
                body: "ok\n".to_string(),
            }),
            access_control: RouteAccessControl::default(),
            rate_limit: None,
        }
    }

    fn host_headers(host: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_str(host).expect("host header should be valid"));
        headers
    }

    fn request_uri(path: &str) -> http::Uri {
        path.parse().expect("request URI should be valid")
    }
}
