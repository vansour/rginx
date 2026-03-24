use std::convert::Infallible;
use std::error::Error as StdError;
use std::net::SocketAddr;
use std::path::Path;
use std::time::Instant;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::{Bytes, BytesMut};
use http::{
    HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri, Version,
    header::{CONTENT_LENGTH, CONTENT_TYPE, HOST, REFERER, USER_AGENT},
};
use http_body_util::BodyExt;
use http_body_util::Full;
use http_body_util::combinators::UnsyncBoxBody;
use hyper::body::{Frame, Incoming, SizeHint};
use hyper::{Request, Response};
use rginx_core::{
    AccessLogFormat, AccessLogValues, ActiveHealthCheck, ConfigSnapshot, Error, Route, RouteAction,
    VirtualHost,
};
use serde::Serialize;

use crate::client_ip::{ClientAddress, resolve_client_address};
use crate::metrics::Metrics;
use crate::proxy::PeerStatusSnapshot;
use crate::router;
use crate::state::{ActiveState, SharedState};

pub(crate) type BoxError = Box<dyn StdError + Send + Sync>;
pub(crate) type HttpBody = UnsyncBoxBody<Bytes, BoxError>;
pub(crate) type HttpResponse = Response<HttpBody>;

const MAX_CONFIG_API_BODY_BYTES: usize = 1024 * 1024;
const CONFIG_API_ALLOW: &str = "GET, HEAD, PUT";

mod access_log;
mod admin;
mod dispatch;
mod grpc;

pub(crate) use admin::{full_body, text_response};
pub use dispatch::handle;
pub(crate) use grpc::{GrpcStatusCode, grpc_error_response};

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};
    use std::time::Duration;

    use base64::Engine as _;
    use bytes::{Bytes, BytesMut};
    use http::{
        HeaderMap, HeaderValue, Response, StatusCode,
        header::{CONTENT_TYPE, HOST},
    };
    use http_body_util::BodyExt;
    use hyper::body::{Frame, SizeHint};
    use rginx_core::{
        AccessLogFormat, ActiveHealthCheck, ConfigSnapshot, GrpcRouteMatch, Route,
        RouteAccessControl, RouteAction, RouteMatcher, RouteRateLimit, RuntimeSettings, Server,
        StaticResponse, Upstream, UpstreamLoadBalance, UpstreamPeer, UpstreamProtocol,
        UpstreamSettings, UpstreamTls, VirtualHost,
    };
    use serde_json::Value;

    use super::access_log::{AccessLogContext, OwnedAccessLogContext, render_access_log_line};
    use super::admin::{metrics_response, status_response};
    use super::dispatch::{
        authorize_route, enforce_rate_limit, response_body_bytes_sent, select_route_for_request,
    };
    use super::grpc::{
        GrpcObservability, GrpcWebObservabilityParser, decode_grpc_web_text_observability_final,
        grpc_observability, grpc_request_metadata, wrap_grpc_observability_response,
    };
    use super::{BoxError, GrpcStatusCode, HttpBody, grpc_error_response, text_response};
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
                    grpc_service: Some("grpc.health.v1.Health".to_string()),
                    interval: Duration::from_secs(5),
                    timeout: Duration::from_secs(2),
                    healthy_successes_required: 2,
                }),
                ..upstream_settings()
            },
        ));
        let config = Arc::new(ConfigSnapshot {
            runtime: RuntimeSettings {
                shutdown_timeout: Duration::from_secs(1),
                worker_threads: None,
                accept_workers: 1,
            },
            server: Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                request_body_read_timeout: None,
                response_write_timeout: None,
                access_log_format: None,
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
        assert_eq!(
            json["upstreams"][0]["active_health_check"]["grpc_service"],
            "grpc.health.v1.Health"
        );
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
            grpc_match: None,
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

        let response = authorize_route(&HeaderMap::new(), &route, &client_address)
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
            runtime: RuntimeSettings {
                shutdown_timeout: Duration::from_secs(1),
                worker_threads: None,
                accept_workers: 1,
            },
            server: Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                request_body_read_timeout: None,
                response_write_timeout: None,
                access_log_format: None,
                tls: None,
            },
            default_vhost: VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: vec![Route {
                    id: "server/routes[0]|exact:/api".to_string(),
                    matcher: RouteMatcher::Exact("/api".to_string()),
                    grpc_match: None,
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

        assert!(
            enforce_rate_limit(
                &HeaderMap::new(),
                &state,
                &route,
                &route.id,
                &client_address,
                &metrics,
            )
            .is_none()
        );

        let response = enforce_rate_limit(
            &HeaderMap::new(),
            &state,
            &route,
            &route.id,
            &client_address,
            &metrics,
        )
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
    fn select_route_for_request_prefers_grpc_specific_route() {
        let config = test_config(
            test_vhost(
                "server",
                Vec::new(),
                vec![
                    test_route("server/routes[0]|prefix:/", RouteMatcher::Prefix("/".to_string())),
                    Route {
                        id: "server/routes[1]|prefix:/|grpc:service=grpc.health.v1.Health,method=Check"
                            .to_string(),
                        matcher: RouteMatcher::Prefix("/".to_string()),
                        grpc_match: Some(GrpcRouteMatch {
                            service: Some("grpc.health.v1.Health".to_string()),
                            method: Some("Check".to_string()),
                        }),
                        action: RouteAction::Static(StaticResponse {
                            status: StatusCode::OK,
                            content_type: "text/plain; charset=utf-8".to_string(),
                            body: "grpc\n".to_string(),
                        }),
                        access_control: RouteAccessControl::default(),
                        rate_limit: None,
                    },
                ],
            ),
            Vec::new(),
        );
        let mut headers = HeaderMap::new();
        headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/grpc"));

        let route = select_route_for_request(
            &config,
            &headers,
            &request_uri("/grpc.health.v1.Health/Check"),
        )
        .expect("gRPC request should match");
        assert_eq!(
            route.id,
            "server/routes[1]|prefix:/|grpc:service=grpc.health.v1.Health,method=Check"
        );
    }

    #[test]
    fn select_route_for_request_does_not_match_grpc_specific_route_for_plain_http_request() {
        let config = test_config(
            test_vhost(
                "server",
                Vec::new(),
                vec![Route {
                    id: "server/routes[0]|prefix:/|grpc:service=grpc.health.v1.Health".to_string(),
                    matcher: RouteMatcher::Prefix("/".to_string()),
                    grpc_match: Some(GrpcRouteMatch {
                        service: Some("grpc.health.v1.Health".to_string()),
                        method: None,
                    }),
                    action: RouteAction::Static(StaticResponse {
                        status: StatusCode::OK,
                        content_type: "text/plain; charset=utf-8".to_string(),
                        body: "grpc\n".to_string(),
                    }),
                    access_control: RouteAccessControl::default(),
                    rate_limit: None,
                }],
            ),
            Vec::new(),
        );

        let route = select_route_for_request(
            &config,
            &HeaderMap::new(),
            &request_uri("/grpc.health.v1.Health/Check"),
        );
        assert!(route.is_none(), "plain HTTP request should not match gRPC-only route");
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

    #[test]
    fn render_access_log_line_uses_configured_template() {
        let format = AccessLogFormat::parse(
            "ACCESS reqid=$request_id status=$status request=\"$request\" grpc=$grpc_protocol svc=$grpc_service rpc=$grpc_method grpc_status=$grpc_status grpc_message=\"$grpc_message\" bytes=$body_bytes_sent ua=\"$http_user_agent\" source=$client_ip_source route=$route",
        )
        .expect("access log format should parse");
        let client_address = ClientAddress {
            peer_addr: "10.0.0.5:4567".parse().unwrap(),
            client_ip: "203.0.113.9".parse().unwrap(),
            forwarded_for: "203.0.113.9".to_string(),
            source: ClientIpSource::XForwardedFor,
        };
        let grpc = GrpcObservability {
            protocol: "grpc-web".to_string(),
            service: "grpc.health.v1.Health".to_string(),
            method: "Check".to_string(),
            status: Some("0".to_string()),
            message: Some("ok".to_string()),
        };

        let rendered = render_access_log_line(
            &format,
            &AccessLogContext {
                request_id: "client-log-42",
                method: "GET",
                host: "app.example.com",
                path: "/demo?x=1",
                request_version: http::Version::HTTP_11,
                user_agent: Some("curl/8.7.1"),
                referer: None,
                client_address: &client_address,
                vhost: "servers[0]",
                route: "servers[0]/routes[0]|exact:/demo",
                status: 200,
                elapsed_ms: 12,
                downstream_scheme: "https",
                body_bytes_sent: Some(3),
                grpc: Some(&grpc),
            },
        );

        assert_eq!(
            rendered,
            "ACCESS reqid=client-log-42 status=200 request=\"GET /demo?x=1 HTTP/1.1\" grpc=grpc-web svc=grpc.health.v1.Health rpc=Check grpc_status=0 grpc_message=\"ok\" bytes=3 ua=\"curl/8.7.1\" source=x_forwarded_for route=servers[0]/routes[0]|exact:/demo"
        );
    }

    #[test]
    fn grpc_observability_extracts_request_and_response_fields() {
        let mut request_headers = HeaderMap::new();
        request_headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/grpc-web+proto"),
        );

        let mut response = text_response(StatusCode::OK, "application/grpc-web+proto", "ok\n");
        response.headers_mut().insert("grpc-status", HeaderValue::from_static("0"));
        response.headers_mut().insert("grpc-message", HeaderValue::from_static("ok"));

        let grpc = grpc_observability(
            grpc_request_metadata(&request_headers, "/grpc.health.v1.Health/Check"),
            response.headers(),
        )
        .expect("grpc metadata should be detected");

        assert_eq!(grpc.protocol, "grpc-web");
        assert_eq!(grpc.service, "grpc.health.v1.Health");
        assert_eq!(grpc.method, "Check");
        assert_eq!(grpc.status.as_deref(), Some("0"));
        assert_eq!(grpc.message.as_deref(), Some("ok"));
    }

    #[test]
    fn grpc_observability_prefers_http_trailers_over_headers() {
        let mut grpc = GrpcObservability {
            protocol: "grpc".to_string(),
            service: "grpc.health.v1.Health".to_string(),
            method: "Check".to_string(),
            status: Some("0".to_string()),
            message: Some("ok".to_string()),
        };
        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", HeaderValue::from_static("14"));
        trailers.insert("grpc-message", HeaderValue::from_static("unavailable"));

        grpc.update_from_headers(&trailers);

        assert_eq!(grpc.status.as_deref(), Some("14"));
        assert_eq!(grpc.message.as_deref(), Some("unavailable"));
    }

    #[test]
    fn grpc_web_observability_parser_extracts_binary_trailers() {
        let mut parser = GrpcWebObservabilityParser::for_protocol("grpc-web")
            .expect("grpc-web parser should be created");
        let mut grpc = GrpcObservability {
            protocol: "grpc-web".to_string(),
            service: "grpc.health.v1.Health".to_string(),
            method: "Check".to_string(),
            status: None,
            message: None,
        };
        let body = grpc_web_observability_body();

        parser.observe_chunk(&body[..7], &mut grpc);
        parser.observe_chunk(&body[7..], &mut grpc);
        parser.finish(&mut grpc);

        assert_eq!(grpc.status.as_deref(), Some("0"));
        assert_eq!(grpc.message.as_deref(), Some("ok"));
    }

    #[test]
    fn grpc_web_observability_parser_extracts_text_trailers() {
        let mut parser = GrpcWebObservabilityParser::for_protocol("grpc-web-text")
            .expect("grpc-web-text parser should be created");
        let mut grpc = GrpcObservability {
            protocol: "grpc-web-text".to_string(),
            service: "grpc.health.v1.Health".to_string(),
            method: "Check".to_string(),
            status: None,
            message: None,
        };
        let encoded =
            base64::engine::general_purpose::STANDARD.encode(grpc_web_observability_body());

        parser.observe_chunk(&encoded.as_bytes()[..9], &mut grpc);
        parser.observe_chunk(&encoded.as_bytes()[9..], &mut grpc);
        parser.finish(&mut grpc);

        assert_eq!(grpc.status.as_deref(), Some("0"));
        assert_eq!(grpc.message.as_deref(), Some("ok"));
    }

    #[test]
    fn grpc_web_text_observability_decoder_handles_chunked_base64() {
        let mut carryover = BytesMut::new();
        carryover.extend_from_slice(b"RA==");
        let tail = decode_grpc_web_text_observability_final(&mut carryover)
            .expect("tail should decode")
            .expect("tail should yield bytes");

        assert_eq!(tail, bytes::Bytes::from_static(b"D"));
    }

    #[test]
    fn grpc_observability_records_cancelled_when_downstream_drops_body_early() {
        let metrics = Metrics::default();
        let response = Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "application/grpc")
            .body(pending_body())
            .expect("response should build");
        let grpc = GrpcObservability {
            protocol: "grpc".to_string(),
            service: "grpc.health.v1.Health".to_string(),
            method: "Check".to_string(),
            status: None,
            message: None,
        };

        let response = wrap_grpc_observability_response(
            response,
            metrics.clone(),
            None,
            test_owned_access_log_context(),
            grpc,
        );
        drop(response);

        let rendered = metrics.render_prometheus();
        assert!(rendered.contains(
            "rginx_grpc_responses_total{route=\"server/routes[0]|exact:/grpc.health.v1.Health/Check\",protocol=\"grpc\",service=\"grpc.health.v1.Health\",method=\"Check\",grpc_status=\"1\"} 1"
        ));
    }

    #[test]
    fn grpc_observability_preserves_existing_status_when_downstream_drops_after_headers() {
        let metrics = Metrics::default();
        let response = Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "application/grpc")
            .header("grpc-status", "0")
            .body(pending_body())
            .expect("response should build");
        let grpc = GrpcObservability {
            protocol: "grpc".to_string(),
            service: "grpc.health.v1.Health".to_string(),
            method: "Check".to_string(),
            status: Some("0".to_string()),
            message: Some("ok".to_string()),
        };

        let response = wrap_grpc_observability_response(
            response,
            metrics.clone(),
            None,
            test_owned_access_log_context(),
            grpc,
        );
        drop(response);

        let rendered = metrics.render_prometheus();
        assert!(rendered.contains(
            "rginx_grpc_responses_total{route=\"server/routes[0]|exact:/grpc.health.v1.Health/Check\",protocol=\"grpc\",service=\"grpc.health.v1.Health\",method=\"Check\",grpc_status=\"0\"} 1"
        ));
        assert!(!rendered.contains("grpc_status=\"1\""));
    }

    #[tokio::test]
    async fn grpc_error_response_builds_trailers_only_http2_error() {
        let mut headers = HeaderMap::new();
        headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/grpc"));

        let response = grpc_error_response(
            &headers,
            GrpcStatusCode::Unavailable,
            "upstream backend is unavailable",
        )
        .expect("gRPC response should be recognized");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").and_then(|value| value.to_str().ok()),
            Some("application/grpc")
        );
        assert_eq!(
            response.headers().get("grpc-status").and_then(|value| value.to_str().ok()),
            Some("14")
        );
        assert_eq!(
            response.headers().get("grpc-message").and_then(|value| value.to_str().ok()),
            Some("upstream backend is unavailable")
        );
        let body = response.into_body().collect().await.expect("body should collect").to_bytes();
        assert!(body.is_empty());
    }

    #[tokio::test]
    async fn grpc_error_response_encodes_grpc_web_text_trailer_block() {
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/grpc-web-text+proto"),
        );

        let response =
            grpc_error_response(&headers, GrpcStatusCode::Unimplemented, "route not found")
                .expect("grpc-web-text response should be recognized");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").and_then(|value| value.to_str().ok()),
            Some("application/grpc-web-text+proto")
        );
        let body = response.into_body().collect().await.expect("body should collect").to_bytes();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(body.as_ref())
            .expect("grpc-web-text body should be valid base64");
        assert_eq!(decoded[0], 0x80);
        let trailer_block =
            std::str::from_utf8(&decoded[5..]).expect("trailer block should be utf-8");
        assert!(trailer_block.contains("grpc-status: 12\r\n"));
        assert!(trailer_block.contains("grpc-message: route not found\r\n"));
    }

    #[test]
    fn authorize_route_returns_grpc_permission_denied_for_grpc_requests() {
        let route = Route {
            id: "server/routes[0]|exact:/metrics".to_string(),
            matcher: RouteMatcher::Exact("/metrics".to_string()),
            grpc_match: None,
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
        let mut headers = HeaderMap::new();
        headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/grpc"));

        let response = authorize_route(&headers, &route, &client_address)
            .expect("non-matching address should be rejected");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("grpc-status").and_then(|value| value.to_str().ok()),
            Some("7")
        );
    }

    #[test]
    fn response_body_bytes_sent_returns_zero_for_head_requests() {
        let response = text_response(StatusCode::OK, "text/plain; charset=utf-8", "hello");
        assert_eq!(response_body_bytes_sent("HEAD", &response), Some(0));
        assert_eq!(response_body_bytes_sent("GET", &response), Some(5));
    }

    fn grpc_web_observability_body() -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x02]);
        body.extend_from_slice(b"ok");

        let trailer_block = b"grpc-status: 0\r\ngrpc-message: ok\r\n";
        body.push(0x80);
        body.extend_from_slice(&(trailer_block.len() as u32).to_be_bytes());
        body.extend_from_slice(trailer_block);
        body
    }

    fn pending_body() -> HttpBody {
        PendingBody.boxed_unsync()
    }

    fn test_owned_access_log_context() -> OwnedAccessLogContext {
        OwnedAccessLogContext {
            request_id: "req-cancel-1".to_string(),
            method: "POST".to_string(),
            host: "grpc.example.com".to_string(),
            path: "/grpc.health.v1.Health/Check".to_string(),
            request_version: http::Version::HTTP_2,
            user_agent: None,
            referer: None,
            client_address: ClientAddress {
                peer_addr: "192.0.2.10:4567".parse().unwrap(),
                client_ip: "192.0.2.10".parse().unwrap(),
                forwarded_for: "192.0.2.10".to_string(),
                source: ClientIpSource::SocketPeer,
            },
            vhost: "server".to_string(),
            route: "server/routes[0]|exact:/grpc.health.v1.Health/Check".to_string(),
            status: 200,
            elapsed_ms: 1,
            downstream_scheme: "https".to_string(),
            body_bytes_sent: None,
        }
    }

    struct PendingBody;

    impl hyper::body::Body for PendingBody {
        type Data = Bytes;
        type Error = BoxError;

        fn poll_frame(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            Poll::Pending
        }

        fn is_end_stream(&self) -> bool {
            false
        }

        fn size_hint(&self) -> SizeHint {
            SizeHint::new()
        }
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
            runtime: RuntimeSettings {
                shutdown_timeout: Duration::from_secs(1),
                worker_threads: None,
                accept_workers: 1,
            },
            server: Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                request_body_read_timeout: None,
                response_write_timeout: None,
                access_log_format: None,
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
            grpc_match: None,
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
