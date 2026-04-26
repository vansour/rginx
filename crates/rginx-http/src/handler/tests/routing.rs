use super::*;

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

#[tokio::test]
async fn handle_serves_requests_for_retired_listener_runtime() {
    let config = test_config(
        test_vhost(
            "server",
            Vec::new(),
            vec![test_route("server/routes[0]|exact:/", RouteMatcher::Exact("/".to_string()))],
        ),
        Vec::new(),
    );
    let retired_source = config.listeners[0].clone();
    let shared = crate::state::SharedState::from_config(config).expect("shared state should build");

    let mut retired = retired_source;
    retired.id = "retired".to_string();
    retired.name = "retired".to_string();
    retired.server.server_header = HeaderValue::from_static("retired-listener");
    shared.retire_listener_runtime(&retired);

    let request = Request::builder()
        .uri("/")
        .body(crate::handler::full_body(""))
        .expect("request should build");
    let connection = std::sync::Arc::new(ConnectionPeerAddrs {
        socket_peer_addr: "192.0.2.10:44321".parse().unwrap(),
        proxy_protocol_source_addr: None,
        tls_client_identity: None,
        tls_version: None,
        tls_alpn: None,
        early_data: false,
    });

    let response = crate::handler::handle(request, shared, connection, "retired").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(http::header::SERVER).and_then(|value| value.to_str().ok()),
        Some("retired-listener")
    );
}

#[tokio::test]
async fn handle_return_route_uses_numeric_body_and_ignores_non_redirect_location() {
    let route = Route {
        id: "server/routes[0]|exact:/custom".to_string(),
        matcher: RouteMatcher::Exact("/custom".to_string()),
        grpc_match: None,
        action: RouteAction::Return(ReturnAction {
            status: StatusCode::from_u16(299).expect("299 should be a valid status"),
            location: "https://example.com/ignored".to_string(),
            body: None,
        }),
        access_control: RouteAccessControl::default(),
        rate_limit: None,
        allow_early_data: false,
        request_buffering: rginx_core::RouteBufferingPolicy::Auto,
        response_buffering: rginx_core::RouteBufferingPolicy::Auto,
        compression: rginx_core::RouteCompressionPolicy::Off,
        compression_min_bytes: None,
        compression_content_types: Vec::new(),
        streaming_response_idle_timeout: None,
    };
    let config = test_config(test_vhost("server", Vec::new(), vec![route]), Vec::new());
    let shared = crate::state::SharedState::from_config(config).expect("shared state should build");

    let request = Request::builder()
        .uri("/custom")
        .body(crate::handler::full_body(""))
        .expect("request should build");
    let connection = std::sync::Arc::new(ConnectionPeerAddrs {
        socket_peer_addr: "192.0.2.10:44322".parse().unwrap(),
        proxy_protocol_source_addr: None,
        tls_client_identity: None,
        tls_version: None,
        tls_alpn: None,
        early_data: false,
    });

    let response = crate::handler::handle(request, shared, connection, "default").await;
    assert_eq!(response.status(), StatusCode::from_u16(299).unwrap());
    assert!(response.headers().get(http::header::LOCATION).is_none());
    let body = response.into_body().collect().await.expect("body should collect").to_bytes();
    assert_eq!(body.as_ref(), b"299\n");
}
