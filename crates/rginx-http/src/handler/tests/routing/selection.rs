use super::super::*;

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
