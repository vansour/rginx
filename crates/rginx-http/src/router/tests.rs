use http::StatusCode;
use rginx_core::{
    GrpcRouteMatch, ReturnAction, Route, RouteAccessControl, RouteAction, RouteMatcher, VirtualHost,
};

use super::{
    RouteMatchContext, select_route, select_route_by_host, select_route_by_host_with_context,
    select_route_with_context, select_vhost,
};

fn make_route(path: &str, body: &str) -> Route {
    Route {
        id: format!("test|prefix:{path}"),
        matcher: RouteMatcher::Prefix(path.to_string()),
        grpc_match: None,
        action: RouteAction::Return(ReturnAction {
            status: StatusCode::OK,
            location: String::new(),
            body: Some(body.to_string()),
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

fn make_vhost(server_names: Vec<&str>, routes: Vec<Route>) -> VirtualHost {
    VirtualHost {
        id: if server_names.is_empty() {
            "server".to_string()
        } else {
            format!("servers[{}]", server_names.join(","))
        },
        server_names: server_names.into_iter().map(String::from).collect(),
        routes,
        tls: None,
    }
}

#[test]
fn exact_routes_beat_prefix_routes() {
    let routes = vec![
        Route {
            id: "test|exact:/api".to_string(),
            matcher: RouteMatcher::Exact("/api".to_string()),
            grpc_match: None,
            action: RouteAction::Return(ReturnAction {
                status: StatusCode::OK,
                location: String::new(),
                body: Some("exact".to_string()),
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
        Route {
            id: "test|prefix:/".to_string(),
            matcher: RouteMatcher::Prefix("/".to_string()),
            grpc_match: None,
            action: RouteAction::Return(ReturnAction {
                status: StatusCode::OK,
                location: String::new(),
                body: Some("prefix".to_string()),
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
    ];

    let route = select_route(&routes, "/api").expect("route should match");
    assert!(matches!(route.matcher, RouteMatcher::Exact(_)));
}

#[test]
fn prefix_routes_respect_segment_boundaries() {
    let routes = vec![Route {
        id: "test|prefix:/api".to_string(),
        matcher: RouteMatcher::Prefix("/api".to_string()),
        grpc_match: None,
        action: RouteAction::Return(ReturnAction {
            status: StatusCode::OK,
            location: String::new(),
            body: Some("prefix".to_string()),
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
    }];

    assert!(select_route(&routes, "/api/demo").is_some());
    assert!(select_route(&routes, "/api").is_some());
    assert!(select_route(&routes, "/apix").is_none());
}

#[test]
fn select_vhost_matches_by_host() {
    let default = make_vhost(vec![], vec![make_route("/", "default")]);
    let api_vhost = make_vhost(vec!["api.example.com"], vec![make_route("/", "api")]);
    let vhosts = vec![api_vhost];

    let selected = select_vhost(&vhosts, &default, "api.example.com");
    assert_eq!(selected.server_names, vec!["api.example.com"]);

    let selected = select_vhost(&vhosts, &default, "unknown.example.com");
    assert!(selected.server_names.is_empty());
}

#[test]
fn select_vhost_matches_wildcard_domains() {
    let default = make_vhost(vec![], vec![make_route("/", "default")]);
    let wildcard_vhost =
        make_vhost(vec!["*.internal.example.com"], vec![make_route("/", "internal")]);
    let vhosts = vec![wildcard_vhost];

    let selected = select_vhost(&vhosts, &default, "app.internal.example.com");
    assert_eq!(selected.server_names, vec!["*.internal.example.com"]);

    let selected = select_vhost(&vhosts, &default, "internal.example.com");
    assert!(selected.server_names.is_empty());

    let selected = select_vhost(&vhosts, &default, "example.com");
    assert!(selected.server_names.is_empty());
}

#[test]
fn select_route_by_host_combines_host_and_path() {
    let default = make_vhost(vec![], vec![make_route("/", "default")]);
    let api_vhost = make_vhost(
        vec!["api.example.com"],
        vec![make_route("/users", "users"), make_route("/", "api-root")],
    );
    let vhosts = vec![api_vhost];

    let result = select_route_by_host(&default, &vhosts, "api.example.com", "/users");
    assert!(result.is_some());
    let (vhost, route) = result.unwrap();
    assert_eq!(vhost.server_names, vec!["api.example.com"]);
    if let RouteAction::Return(resp) = &route.action {
        assert_eq!(resp.body.as_deref(), Some("users"));
    } else {
        panic!("expected return response");
    }

    let result = select_route_by_host(&default, &vhosts, "unknown.example.com", "/");
    assert!(result.is_some());
    let (vhost, _) = result.unwrap();
    assert!(vhost.server_names.is_empty());
}

#[test]
fn grpc_specific_routes_beat_generic_routes_for_same_path() {
    let routes = vec![
        Route {
            id: "test|prefix:/|grpc:service=grpc.health.v1.Health,method=Check".to_string(),
            matcher: RouteMatcher::Prefix("/".to_string()),
            grpc_match: Some(GrpcRouteMatch {
                service: Some("grpc.health.v1.Health".to_string()),
                method: Some("Check".to_string()),
            }),
            action: RouteAction::Return(ReturnAction {
                status: StatusCode::OK,
                location: String::new(),
                body: Some("grpc".to_string()),
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
        Route {
            id: "test|prefix:/".to_string(),
            matcher: RouteMatcher::Prefix("/".to_string()),
            grpc_match: None,
            action: RouteAction::Return(ReturnAction {
                status: StatusCode::OK,
                location: String::new(),
                body: Some("generic".to_string()),
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
    ];

    let route = select_route_with_context(
        &routes,
        &RouteMatchContext::with_grpc("/", "grpc.health.v1.Health", "Check"),
    )
    .expect("gRPC route should match");
    assert_eq!(route.id, "test|prefix:/|grpc:service=grpc.health.v1.Health,method=Check");
}

#[test]
fn grpc_specific_routes_require_grpc_request_context() {
    let routes = vec![Route {
        id: "test|prefix:/|grpc:service=grpc.health.v1.Health".to_string(),
        matcher: RouteMatcher::Prefix("/".to_string()),
        grpc_match: Some(GrpcRouteMatch {
            service: Some("grpc.health.v1.Health".to_string()),
            method: None,
        }),
        action: RouteAction::Return(ReturnAction {
            status: StatusCode::OK,
            location: String::new(),
            body: Some("grpc".to_string()),
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
    }];

    assert!(select_route(&routes, "/").is_none());
    assert!(
        select_route_with_context(
            &routes,
            &RouteMatchContext::with_grpc("/", "grpc.health.v1.Health", "Check"),
        )
        .is_some()
    );
}

#[test]
fn select_route_by_host_with_context_respects_grpc_constraints() {
    let default = make_vhost(vec![], vec![make_route("/", "default")]);
    let api_vhost = make_vhost(
        vec!["api.example.com"],
        vec![
            Route {
                id: "test|prefix:/|grpc:service=grpc.health.v1.Health".to_string(),
                matcher: RouteMatcher::Prefix("/".to_string()),
                grpc_match: Some(GrpcRouteMatch {
                    service: Some("grpc.health.v1.Health".to_string()),
                    method: None,
                }),
                action: RouteAction::Return(ReturnAction {
                    status: StatusCode::OK,
                    location: String::new(),
                    body: Some("grpc".to_string()),
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
            make_route("/", "fallback"),
        ],
    );
    let vhosts = vec![api_vhost];

    let result = select_route_by_host_with_context(
        &default,
        &vhosts,
        "api.example.com",
        &RouteMatchContext::with_grpc("/", "grpc.health.v1.Health", "Check"),
    )
    .expect("gRPC route should match");
    assert_eq!(result.1.id, "test|prefix:/|grpc:service=grpc.health.v1.Health");
}
