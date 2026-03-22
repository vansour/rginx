use rginx_core::{Route, VirtualHost};

pub fn select_route<'a>(routes: &'a [Route], path: &str) -> Option<&'a Route> {
    routes.iter().find(|route| route.matcher.matches(path))
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

#[cfg(test)]
mod tests {
    use http::StatusCode;
    use rginx_core::{
        Route, RouteAccessControl, RouteAction, RouteMatcher, StaticResponse, VirtualHost,
    };

    use super::{select_route, select_route_by_host, select_vhost};

    fn make_route(path: &str, body: &str) -> Route {
        Route {
            id: format!("test|prefix:{path}"),
            matcher: RouteMatcher::Prefix(path.to_string()),
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
}
