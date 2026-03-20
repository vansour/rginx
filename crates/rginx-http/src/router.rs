use rginx_core::Route;

pub fn select_route<'a>(routes: &'a [Route], path: &str) -> Option<&'a Route> {
    routes.iter().find(|route| route.matcher.matches(path))
}

#[cfg(test)]
mod tests {
    use http::StatusCode;
    use rginx_core::{Route, RouteAction, RouteMatcher, StaticResponse};

    use super::select_route;

    #[test]
    fn exact_routes_beat_prefix_routes() {
        let routes = vec![
            Route {
                matcher: RouteMatcher::Exact("/api".to_string()),
                action: RouteAction::Static(StaticResponse {
                    status: StatusCode::OK,
                    content_type: "text/plain".to_string(),
                    body: "exact".to_string(),
                }),
            },
            Route {
                matcher: RouteMatcher::Prefix("/".to_string()),
                action: RouteAction::Static(StaticResponse {
                    status: StatusCode::OK,
                    content_type: "text/plain".to_string(),
                    body: "prefix".to_string(),
                }),
            },
        ];

        let route = select_route(&routes, "/api").expect("route should match");
        assert!(matches!(route.matcher, RouteMatcher::Exact(_)));
    }

    #[test]
    fn prefix_routes_respect_segment_boundaries() {
        let routes = vec![Route {
            matcher: RouteMatcher::Prefix("/api".to_string()),
            action: RouteAction::Static(StaticResponse {
                status: StatusCode::OK,
                content_type: "text/plain".to_string(),
                body: "prefix".to_string(),
            }),
        }];

        assert!(select_route(&routes, "/api/demo").is_some());
        assert!(select_route(&routes, "/api").is_some());
        assert!(select_route(&routes, "/apix").is_none());
    }
}
