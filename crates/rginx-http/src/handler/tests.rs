use std::collections::HashMap;
use std::time::Duration;

use base64::Engine as _;
use bytes::BytesMut;
use http::{HeaderMap, HeaderValue, Request, StatusCode, header::HOST};
use http_body_util::BodyExt;
use rginx_core::{
    AccessLogFormat, ConfigSnapshot, GrpcRouteMatch, ReturnAction, Route, RouteAccessControl,
    RouteAction, RouteMatcher, RuntimeSettings, Server, VirtualHost, default_server_header,
};

use super::access_log::{AccessLogContext, render_access_log_line};
use super::dispatch::{
    authorize_route, finalize_downstream_response, response_body_bytes_sent,
    select_route_for_request,
};
use super::grpc::{
    GrpcObservability, GrpcWebObservabilityParser, decode_grpc_web_text_observability_final,
    grpc_observability, grpc_request_metadata,
};
use super::{GrpcStatusCode, attach_connection_metadata, grpc_error_response, text_response};
use crate::client_ip::{ClientAddress, ClientIpSource, ConnectionPeerAddrs, TlsClientIdentity};
use crate::compression::ResponseCompressionOptions;

#[test]
fn authorize_route_rejects_disallowed_remote_addr() {
    let route = Route {
        id: "server/routes[0]|exact:/protected".to_string(),
        matcher: RouteMatcher::Exact("/protected".to_string()),
        grpc_match: None,
        action: RouteAction::Return(ReturnAction {
            status: StatusCode::OK,
            location: String::new(),
            body: Some("ok\n".to_string()),
        }),
        access_control: RouteAccessControl::new(
            vec!["127.0.0.1/32".parse().unwrap(), "::1/128".parse().unwrap()],
            Vec::new(),
        ),
        rate_limit: None,
        allow_early_data: false,
        request_buffering: rginx_core::RouteBufferingPolicy::Auto,
        response_buffering: rginx_core::RouteBufferingPolicy::Auto,
        compression: rginx_core::RouteCompressionPolicy::Auto,
        compression_min_bytes: None,
        compression_content_types: Vec::new(),
        streaming_response_idle_timeout: None,
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
async fn select_route_for_request_uses_host_specific_vhost_routes() {
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

    let route =
        select_route_for_request(&config, &host_headers("api.example.com"), &request_uri("/users"))
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
            vec![test_route("servers[0]/routes[0]|exact:/", RouteMatcher::Exact("/".to_string()))],
        )],
    );

    let route =
        select_route_for_request(&config, &host_headers("unknown.example.com"), &request_uri("/"))
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
fn select_route_for_request_prefers_exact_host_over_wildcard_host() {
    let config = test_config(
        test_vhost("server", Vec::new(), Vec::new()),
        vec![
            test_vhost(
                "servers[0]",
                vec!["*.example.com"],
                vec![test_route(
                    "servers[0]/routes[0]|exact:/",
                    RouteMatcher::Exact("/".to_string()),
                )],
            ),
            test_vhost(
                "servers[1]",
                vec!["api.example.com"],
                vec![test_route(
                    "servers[1]/routes[0]|exact:/",
                    RouteMatcher::Exact("/".to_string()),
                )],
            ),
        ],
    );

    let route =
        select_route_for_request(&config, &host_headers("api.example.com"), &request_uri("/"))
            .expect("exact host should match");
    assert_eq!(route.id, "servers[1]/routes[0]|exact:/");
}

#[test]
fn select_route_for_request_prefers_more_specific_wildcard_host() {
    let config = test_config(
        test_vhost("server", Vec::new(), Vec::new()),
        vec![
            test_vhost(
                "servers[0]",
                vec!["*.example.com"],
                vec![test_route(
                    "servers[0]/routes[0]|exact:/",
                    RouteMatcher::Exact("/".to_string()),
                )],
            ),
            test_vhost(
                "servers[1]",
                vec!["*.api.example.com"],
                vec![test_route(
                    "servers[1]/routes[0]|exact:/",
                    RouteMatcher::Exact("/".to_string()),
                )],
            ),
        ],
    );

    let route =
        select_route_for_request(&config, &host_headers("edge.api.example.com"), &request_uri("/"))
            .expect("more specific wildcard should match");
    assert_eq!(route.id, "servers[1]/routes[0]|exact:/");
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
                    action: RouteAction::Return(ReturnAction {
                        status: StatusCode::OK,
                        location: String::new(),
                        body: Some("grpc\n".to_string()),
                    }),
                    access_control: RouteAccessControl::default(),
                    rate_limit: None,
                    allow_early_data: false,
                    request_buffering: rginx_core::RouteBufferingPolicy::Auto,
                    response_buffering: rginx_core::RouteBufferingPolicy::Auto,
                    compression: rginx_core::RouteCompressionPolicy::Auto,
                    compression_min_bytes: None,
                    compression_content_types: Vec::new(),
                    streaming_response_idle_timeout: None,
                },
            ],
        ),
        Vec::new(),
    );
    let mut headers = HeaderMap::new();
    headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/grpc"));

    let route =
        select_route_for_request(&config, &headers, &request_uri("/grpc.health.v1.Health/Check"))
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
                action: RouteAction::Return(ReturnAction {
                    status: StatusCode::OK,
                    location: String::new(),
                    body: Some("grpc\n".to_string()),
                }),
                access_control: RouteAccessControl::default(),
                rate_limit: None,
                allow_early_data: false,
                request_buffering: rginx_core::RouteBufferingPolicy::Auto,
                response_buffering: rginx_core::RouteBufferingPolicy::Auto,
                compression: rginx_core::RouteCompressionPolicy::Auto,
                compression_min_bytes: None,
                compression_content_types: Vec::new(),
                streaming_response_idle_timeout: None,
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

    let route =
        select_route_for_request(&config, &host_headers("api.example.com"), &request_uri("/users"));
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
            tls_version: Some("TLS1.3"),
            tls_alpn: Some("h2"),
            body_bytes_sent: Some(3),
            tls_client_identity: None,
            grpc: Some(&grpc),
        },
    );

    assert_eq!(
        rendered,
        "ACCESS reqid=client-log-42 status=200 request=\"GET /demo?x=1 HTTP/1.1\" grpc=grpc-web svc=grpc.health.v1.Health rpc=Check grpc_status=0 grpc_message=\"ok\" bytes=3 ua=\"curl/8.7.1\" source=x_forwarded_for route=servers[0]/routes[0]|exact:/demo"
    );
}

#[test]
fn attach_connection_metadata_inserts_tls_client_identity_extension() {
    let mut request = Request::builder().uri("http://example.com/").body(()).unwrap();
    let connection = ConnectionPeerAddrs {
        socket_peer_addr: "127.0.0.1:44321".parse().unwrap(),
        proxy_protocol_source_addr: None,
        tls_client_identity: Some(TlsClientIdentity {
            subject: Some("CN=test-client".to_string()),
            issuer: Some("CN=test-ca".to_string()),
            serial_number: Some("01".to_string()),
            san_dns_names: vec!["client.example.com".to_string()],
            chain_length: 2,
            chain_subjects: vec!["CN=test-client".to_string(), "CN=test-ca".to_string()],
        }),
        tls_version: Some("TLS1.3".to_string()),
        tls_alpn: Some("h2".to_string()),
        early_data: false,
    };

    attach_connection_metadata(&mut request, &connection);

    let identity = request
        .extensions()
        .get::<TlsClientIdentity>()
        .expect("TLS client identity should be attached");
    assert_eq!(identity.subject.as_deref(), Some("CN=test-client"));
    assert_eq!(identity.san_dns_names, vec!["client.example.com"]);
}

#[test]
fn grpc_observability_extracts_request_and_response_fields() {
    let mut request_headers = HeaderMap::new();
    request_headers
        .insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/grpc-web+proto"));

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
    let encoded = base64::engine::general_purpose::STANDARD.encode(grpc_web_observability_body());

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

    let response = grpc_error_response(&headers, GrpcStatusCode::Unimplemented, "route not found")
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
    let trailer_block = std::str::from_utf8(&decoded[5..]).expect("trailer block should be utf-8");
    assert!(trailer_block.contains("grpc-status: 12\r\n"));
    assert!(trailer_block.contains("grpc-message: route not found\r\n"));
}

#[test]
fn authorize_route_returns_grpc_permission_denied_for_grpc_requests() {
    let route = Route {
        id: "server/routes[0]|exact:/protected".to_string(),
        matcher: RouteMatcher::Exact("/protected".to_string()),
        grpc_match: None,
        action: RouteAction::Return(ReturnAction {
            status: StatusCode::OK,
            location: String::new(),
            body: Some("ok\n".to_string()),
        }),
        access_control: RouteAccessControl::new(
            vec!["127.0.0.1/32".parse().unwrap(), "::1/128".parse().unwrap()],
            Vec::new(),
        ),
        rate_limit: None,
        allow_early_data: false,
        request_buffering: rginx_core::RouteBufferingPolicy::Auto,
        response_buffering: rginx_core::RouteBufferingPolicy::Auto,
        compression: rginx_core::RouteCompressionPolicy::Auto,
        compression_min_bytes: None,
        compression_content_types: Vec::new(),
        streaming_response_idle_timeout: None,
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

#[tokio::test]
async fn finalize_downstream_response_compresses_plain_text_responses() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(http::header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let compression_options = ResponseCompressionOptions::default();

    let finalized = finalize_downstream_response(
        &http::Method::GET,
        &request_headers,
        &compression_options,
        HeaderValue::from_static("req-plain"),
        text_response(
            StatusCode::OK,
            "text/plain; charset=utf-8",
            "hello compression pipeline\n".repeat(32),
        ),
        None,
        None,
        default_server_header(),
    )
    .await;

    assert!(finalized.grpc.is_none());
    assert_eq!(
        finalized
            .response
            .headers()
            .get(http::header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok()),
        Some("gzip")
    );
    assert_eq!(
        finalized.response.headers().get("x-request-id").and_then(|value| value.to_str().ok()),
        Some("req-plain")
    );
}

#[tokio::test]
async fn finalize_downstream_response_skips_compression_for_grpc_responses() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(http::header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    request_headers
        .insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/grpc"));
    let compression_options = ResponseCompressionOptions::default();

    let finalized = finalize_downstream_response(
        &http::Method::POST,
        &request_headers,
        &compression_options,
        HeaderValue::from_static("req-grpc"),
        text_response(StatusCode::OK, "application/grpc", "hello grpc pipeline\n".repeat(32)),
        grpc_request_metadata(&request_headers, "/grpc.health.v1.Health/Check"),
        None,
        default_server_header(),
    )
    .await;

    assert!(finalized.grpc.is_some());
    assert!(finalized.response.headers().get(http::header::CONTENT_ENCODING).is_none());
    assert_eq!(
        finalized.response.headers().get("x-request-id").and_then(|value| value.to_str().ok()),
        Some("req-grpc")
    );
}

#[tokio::test]
async fn finalize_downstream_response_strips_head_body_after_final_transforms() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(http::header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let compression_options = ResponseCompressionOptions::default();

    let finalized = finalize_downstream_response(
        &http::Method::HEAD,
        &request_headers,
        &compression_options,
        HeaderValue::from_static("req-head"),
        text_response(
            StatusCode::OK,
            "text/plain; charset=utf-8",
            "hello head pipeline\n".repeat(32),
        ),
        None,
        None,
        default_server_header(),
    )
    .await;

    assert!(finalized.grpc.is_none());
    assert!(finalized.response.headers().get(http::header::CONTENT_ENCODING).is_none());
    assert_eq!(finalized.body_bytes_sent, Some(0));
    let content_length = finalized
        .response
        .headers()
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .expect("HEAD response should preserve content length")
        .parse::<usize>()
        .expect("content length should parse");
    let body = finalized
        .response
        .into_body()
        .collect()
        .await
        .expect("HEAD body should collect")
        .to_bytes();
    assert!(content_length > 0);
    assert!(body.is_empty());
}

#[tokio::test]
async fn finalize_downstream_response_injects_alt_svc_when_provided() {
    let request_headers = HeaderMap::new();
    let compression_options = ResponseCompressionOptions::default();

    let finalized = finalize_downstream_response(
        &http::Method::GET,
        &request_headers,
        &compression_options,
        HeaderValue::from_static("req-alt-svc"),
        text_response(StatusCode::OK, "text/plain; charset=utf-8", "hello"),
        None,
        Some(HeaderValue::from_static("h3=\":443\"; ma=7200")),
        HeaderValue::from_static("edge-test"),
    )
    .await;

    assert_eq!(
        finalized
            .response
            .headers()
            .get(http::header::ALT_SVC)
            .and_then(|value| value.to_str().ok()),
        Some("h3=\":443\"; ma=7200")
    );
    assert_eq!(
        finalized
            .response
            .headers()
            .get(http::header::SERVER)
            .and_then(|value| value.to_str().ok()),
        Some("edge-test")
    );
    assert!(finalized.response.headers().get(http::header::DATE).is_some());
}

#[tokio::test]
async fn finalize_downstream_response_preserves_existing_date_header() {
    let request_headers = HeaderMap::new();
    let compression_options = ResponseCompressionOptions::default();
    let upstream_date = HeaderValue::from_static("Tue, 15 Nov 1994 08:12:31 GMT");
    let mut response = text_response(StatusCode::OK, "text/plain; charset=utf-8", "hello");
    response.headers_mut().insert(http::header::DATE, upstream_date.clone());

    let finalized = finalize_downstream_response(
        &http::Method::GET,
        &request_headers,
        &compression_options,
        HeaderValue::from_static("req-date"),
        response,
        None,
        None,
        default_server_header(),
    )
    .await;

    assert_eq!(finalized.response.headers().get(http::header::DATE), Some(&upstream_date));
}

#[tokio::test]
async fn finalize_downstream_response_respects_response_buffering_off() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(http::header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let compression_options = ResponseCompressionOptions {
        response_buffering: rginx_core::RouteBufferingPolicy::Off,
        ..ResponseCompressionOptions::default()
    };

    let finalized = finalize_downstream_response(
        &http::Method::GET,
        &request_headers,
        &compression_options,
        HeaderValue::from_static("req-stream"),
        text_response(
            StatusCode::OK,
            "text/plain; charset=utf-8",
            "hello response buffering\n".repeat(32),
        ),
        None,
        None,
        default_server_header(),
    )
    .await;

    assert!(finalized.response.headers().get(http::header::CONTENT_ENCODING).is_none());
}

#[tokio::test]
async fn finalize_downstream_response_force_compresses_small_responses() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(http::header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let compression_options = ResponseCompressionOptions {
        compression: rginx_core::RouteCompressionPolicy::Force,
        ..ResponseCompressionOptions::default()
    };

    let finalized = finalize_downstream_response(
        &http::Method::GET,
        &request_headers,
        &compression_options,
        HeaderValue::from_static("req-force"),
        text_response(StatusCode::OK, "text/plain; charset=utf-8", "a".repeat(128)),
        None,
        None,
        default_server_header(),
    )
    .await;

    assert_eq!(
        finalized
            .response
            .headers()
            .get(http::header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok()),
        Some("gzip")
    );
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

fn test_config(default_vhost: VirtualHost, vhosts: Vec<VirtualHost>) -> ConfigSnapshot {
    let server = Server {
        listen_addr: "127.0.0.1:8080".parse().unwrap(),
        server_header: rginx_core::default_server_header(),
        default_certificate: None,
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
    };
    ConfigSnapshot {
        runtime: RuntimeSettings {
            shutdown_timeout: Duration::from_secs(1),
            worker_threads: None,
            accept_workers: 1,
        },
        listeners: vec![rginx_core::Listener {
            id: "default".to_string(),
            name: "default".to_string(),
            server,
            tls_termination_enabled: false,
            proxy_protocol_enabled: false,
            http3: None,
        }],
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
        action: RouteAction::Return(ReturnAction {
            status: StatusCode::OK,
            location: String::new(),
            body: Some("ok\n".to_string()),
        }),
        access_control: RouteAccessControl::default(),
        rate_limit: None,
        allow_early_data: false,
        request_buffering: rginx_core::RouteBufferingPolicy::Auto,
        response_buffering: rginx_core::RouteBufferingPolicy::Auto,
        compression: rginx_core::RouteCompressionPolicy::Auto,
        compression_min_bytes: None,
        compression_content_types: Vec::new(),
        streaming_response_idle_timeout: None,
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
