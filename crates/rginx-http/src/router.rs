use rginx_core::{Route, VirtualHost};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrpcRequestMatch<'a> {
    pub service: &'a str,
    pub method: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteMatchContext<'a> {
    pub path: &'a str,
    pub grpc: Option<GrpcRequestMatch<'a>>,
}

impl<'a> RouteMatchContext<'a> {
    pub fn new(path: &'a str) -> Self {
        Self { path, grpc: None }
    }

    pub fn with_grpc(path: &'a str, service: &'a str, method: &'a str) -> Self {
        Self { path, grpc: Some(GrpcRequestMatch { service, method }) }
    }
}

pub fn select_route<'a>(routes: &'a [Route], path: &str) -> Option<&'a Route> {
    select_route_with_context(routes, &RouteMatchContext::new(path))
}

pub fn select_route_with_context<'a>(
    routes: &'a [Route],
    context: &RouteMatchContext<'_>,
) -> Option<&'a Route> {
    routes.iter().filter(|route| route_matches(route, context)).fold(None, |selected, route| {
        match selected {
            None => Some(route),
            Some(current) if route.priority() > current.priority() => Some(route),
            Some(current) => Some(current),
        }
    })
}

/// 根据 Host 选择虚拟主机
pub fn select_vhost<'a>(
    vhosts: &'a [VirtualHost],
    default: &'a VirtualHost,
    host: &str,
) -> &'a VirtualHost {
    vhosts.iter().find(|vhost| vhost.matches_host(host)).unwrap_or(default)
}

/// 在指定虚拟主机内选择路由
pub fn select_route_in_vhost<'a>(vhost: &'a VirtualHost, path: &str) -> Option<&'a Route> {
    select_route(&vhost.routes, path)
}

pub fn select_route_in_vhost_with_context<'a>(
    vhost: &'a VirtualHost,
    context: &RouteMatchContext<'_>,
) -> Option<&'a Route> {
    select_route_with_context(&vhost.routes, context)
}

/// 组合：Host + Path 双层匹配
pub fn select_route_by_host<'a>(
    default_vhost: &'a VirtualHost,
    vhosts: &'a [VirtualHost],
    host: &str,
    path: &str,
) -> Option<(&'a VirtualHost, &'a Route)> {
    let vhost = select_vhost(vhosts, default_vhost, host);
    select_route_in_vhost(vhost, path).map(|route| (vhost, route))
}

pub fn select_route_by_host_with_context<'a>(
    default_vhost: &'a VirtualHost,
    vhosts: &'a [VirtualHost],
    host: &str,
    context: &RouteMatchContext<'_>,
) -> Option<(&'a VirtualHost, &'a Route)> {
    let vhost = select_vhost(vhosts, default_vhost, host);
    select_route_in_vhost_with_context(vhost, context).map(|route| (vhost, route))
}

fn route_matches(route: &Route, context: &RouteMatchContext<'_>) -> bool {
    if !route.matcher.matches(context.path) {
        return false;
    }

    route.grpc_match.as_ref().is_none_or(|grpc_match| {
        context.grpc.is_some_and(|grpc| grpc_match.matches(grpc.service, grpc.method))
    })
}

#[cfg(test)]
mod tests {
    use http::StatusCode;
    use rginx_core::{
        GrpcRouteMatch, Route, RouteAccessControl, RouteAction, RouteMatcher, StaticResponse,
        VirtualHost,
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
            action: RouteAction::Static(StaticResponse {
                status: StatusCode::OK,
                content_type: "text/plain".to_string(),
                body: body.to_string(),
            }),
            access_control: RouteAccessControl::default(),
            rate_limit: None,
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
                action: RouteAction::Static(StaticResponse {
                    status: StatusCode::OK,
                    content_type: "text/plain".to_string(),
                    body: "exact".to_string(),
                }),
                access_control: RouteAccessControl::default(),
                rate_limit: None,
            },
            Route {
                id: "test|prefix:/".to_string(),
                matcher: RouteMatcher::Prefix("/".to_string()),
                grpc_match: None,
                action: RouteAction::Static(StaticResponse {
                    status: StatusCode::OK,
                    content_type: "text/plain".to_string(),
                    body: "prefix".to_string(),
                }),
                access_control: RouteAccessControl::default(),
                rate_limit: None,
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
            action: RouteAction::Static(StaticResponse {
                status: StatusCode::OK,
                content_type: "text/plain".to_string(),
                body: "prefix".to_string(),
            }),
            access_control: RouteAccessControl::default(),
            rate_limit: None,
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
        assert_eq!(selected.server_names, vec!["*.internal.example.com"]);

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
        if let RouteAction::Static(resp) = &route.action {
            assert_eq!(resp.body, "users");
        } else {
            panic!("expected static response");
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
                action: RouteAction::Static(StaticResponse {
                    status: StatusCode::OK,
                    content_type: "text/plain".to_string(),
                    body: "grpc".to_string(),
                }),
                access_control: RouteAccessControl::default(),
                rate_limit: None,
            },
            Route {
                id: "test|prefix:/".to_string(),
                matcher: RouteMatcher::Prefix("/".to_string()),
                grpc_match: None,
                action: RouteAction::Static(StaticResponse {
                    status: StatusCode::OK,
                    content_type: "text/plain".to_string(),
                    body: "generic".to_string(),
                }),
                access_control: RouteAccessControl::default(),
                rate_limit: None,
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
            action: RouteAction::Static(StaticResponse {
                status: StatusCode::OK,
                content_type: "text/plain".to_string(),
                body: "grpc".to_string(),
            }),
            access_control: RouteAccessControl::default(),
            rate_limit: None,
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
                    action: RouteAction::Static(StaticResponse {
                        status: StatusCode::OK,
                        content_type: "text/plain".to_string(),
                        body: "grpc".to_string(),
                    }),
                    access_control: RouteAccessControl::default(),
                    rate_limit: None,
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
}
