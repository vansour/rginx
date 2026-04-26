use super::super::*;

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
