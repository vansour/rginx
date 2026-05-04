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
    let mut exact = None::<Candidate<'a>>;
    let mut prefix = None::<Candidate<'a>>;

    for (index, route) in routes.iter().enumerate() {
        if !route_matches(route, context) {
            continue;
        }

        match &route.matcher {
            rginx_core::RouteMatcher::Exact(path) => {
                exact =
                    select_more_specific(exact, Candidate::new(index, route, path.len(), false));
            }
            rginx_core::RouteMatcher::PreferredPrefix(path) => {
                prefix =
                    select_more_specific(prefix, Candidate::new(index, route, path.len(), true));
            }
            rginx_core::RouteMatcher::Prefix(path) => {
                prefix =
                    select_more_specific(prefix, Candidate::new(index, route, path.len(), false));
            }
            rginx_core::RouteMatcher::Regex(_) => {}
        }
    }

    if let Some(candidate) = exact {
        return Some(candidate.route);
    }

    if let Some(candidate) = prefix
        && candidate.preferred_prefix
    {
        return Some(candidate.route);
    }

    for route in routes {
        if matches!(route.matcher, rginx_core::RouteMatcher::Regex(_))
            && route_matches(route, context)
        {
            return Some(route);
        }
    }

    prefix.map(|candidate| candidate.route)
}

#[derive(Clone, Copy)]
struct Candidate<'a> {
    index: usize,
    route: &'a Route,
    match_len: usize,
    grpc_rank: u8,
    preferred_prefix: bool,
}

impl<'a> Candidate<'a> {
    fn new(index: usize, route: &'a Route, match_len: usize, preferred_prefix: bool) -> Self {
        Self {
            index,
            route,
            match_len,
            grpc_rank: route.grpc_match.as_ref().map_or(0, |grpc_match| grpc_match.priority()),
            preferred_prefix,
        }
    }
}

fn select_more_specific<'a>(
    current: Option<Candidate<'a>>,
    candidate: Candidate<'a>,
) -> Option<Candidate<'a>> {
    match current {
        None => Some(candidate),
        Some(existing) if candidate.match_len > existing.match_len => Some(candidate),
        Some(existing) if candidate.match_len < existing.match_len => Some(existing),
        Some(existing) if candidate.preferred_prefix && !existing.preferred_prefix => {
            Some(candidate)
        }
        Some(existing) if !candidate.preferred_prefix && existing.preferred_prefix => {
            Some(existing)
        }
        Some(existing) if candidate.grpc_rank > existing.grpc_rank => Some(candidate),
        Some(existing) if candidate.grpc_rank < existing.grpc_rank => Some(existing),
        Some(existing) if candidate.index < existing.index => Some(candidate),
        Some(existing) => Some(existing),
    }
}

/// 根据 Host 选择虚拟主机
pub fn select_vhost<'a>(
    vhosts: &'a [VirtualHost],
    default: &'a VirtualHost,
    host: &str,
) -> &'a VirtualHost {
    let mut selected = None::<((u8, usize), &VirtualHost)>;

    for vhost in vhosts {
        let Some(matched) = vhost.best_server_name_match(host) else {
            continue;
        };
        let priority = matched.priority();
        match selected {
            None => selected = Some((priority, vhost)),
            Some((current_priority, _)) if priority > current_priority => {
                selected = Some((priority, vhost))
            }
            Some(_) => {}
        }
    }

    selected.map(|(_, vhost)| vhost).unwrap_or(default)
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
mod tests;
