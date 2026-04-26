use super::super::*;

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
