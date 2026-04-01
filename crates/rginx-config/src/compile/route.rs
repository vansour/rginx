use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use http::StatusCode;
use ipnet::IpNet;
use rginx_core::{
    Error, FileTarget, GrpcRouteMatch, ProxyTarget, Result, ReturnAction, Route,
    RouteAccessControl, RouteAction, RouteMatcher, RouteRateLimit, StaticResponse, Upstream,
};

use crate::model::{HandlerConfig, LocationConfig, MatcherConfig};

pub(super) fn compile_routes(
    locations: Vec<LocationConfig>,
    upstreams: &HashMap<String, Arc<Upstream>>,
    base_dir: &Path,
    vhost_id: &str,
) -> Result<Vec<Route>> {
    let mut routes = locations
        .into_iter()
        .enumerate()
        .map(|(route_index, location)| {
            compile_route(location, route_index, upstreams, base_dir, vhost_id)
        })
        .collect::<Result<Vec<_>>>()?;

    routes.sort_by_key(|route| std::cmp::Reverse(route.priority()));

    Ok(routes)
}

fn compile_route(
    location: LocationConfig,
    route_index: usize,
    upstreams: &HashMap<String, Arc<Upstream>>,
    base_dir: &Path,
    vhost_id: &str,
) -> Result<Route> {
    let LocationConfig {
        matcher,
        handler,
        grpc_service,
        grpc_method,
        allow_cidrs,
        deny_cidrs,
        requests_per_sec,
        burst,
    } = location;

    let matcher = match matcher {
        MatcherConfig::Exact(path) => RouteMatcher::Exact(path),
        MatcherConfig::Prefix(path) => RouteMatcher::Prefix(path),
    };
    let grpc_match = if grpc_service.is_some() || grpc_method.is_some() {
        Some(GrpcRouteMatch { service: grpc_service, method: grpc_method })
    } else {
        None
    };
    let route_id = if let Some(grpc_match) = &grpc_match {
        format!(
            "{vhost_id}/routes[{route_index}]|{}|{}",
            matcher.id_fragment(),
            grpc_match.id_fragment()
        )
    } else {
        format!("{vhost_id}/routes[{route_index}]|{}", matcher.id_fragment())
    };
    let access_control = compile_route_access_control(&matcher, allow_cidrs, deny_cidrs)?;
    let rate_limit = compile_route_rate_limit(&matcher, requests_per_sec, burst)?;
    let action = compile_route_action(handler, upstreams, base_dir)?;

    Ok(Route { id: route_id, matcher, grpc_match, action, access_control, rate_limit })
}

fn compile_route_action(
    handler: HandlerConfig,
    upstreams: &HashMap<String, Arc<Upstream>>,
    base_dir: &Path,
) -> Result<RouteAction> {
    match handler {
        HandlerConfig::Static { status, content_type, body } => {
            Ok(RouteAction::Static(StaticResponse {
                status: StatusCode::from_u16(status.unwrap_or(200))?,
                content_type: content_type
                    .unwrap_or_else(|| "text/plain; charset=utf-8".to_string()),
                body,
            }))
        }
        HandlerConfig::Proxy { upstream, preserve_host, strip_prefix, proxy_set_headers } => {
            let compiled = upstreams.get(&upstream).cloned().ok_or_else(|| {
                Error::Config(format!("proxy upstream `{upstream}` is not defined"))
            })?;

            let proxy_set_headers = proxy_set_headers
                .into_iter()
                .map(|(name, value)| {
                    let header_name = name
                        .parse::<http::header::HeaderName>()
                        .map_err(|e| Error::Config(format!("invalid header name `{name}`: {e}")))?;
                    let header_value = value.parse::<http::header::HeaderValue>().map_err(|e| {
                        Error::Config(format!("invalid header value for `{name}`: {e}"))
                    })?;
                    Ok((header_name, header_value))
                })
                .collect::<Result<Vec<_>>>()?;

            Ok(RouteAction::Proxy(ProxyTarget {
                upstream_name: upstream,
                upstream: compiled,
                preserve_host: preserve_host.unwrap_or(false),
                strip_prefix: strip_prefix.and_then(|s| if s.is_empty() { None } else { Some(s) }),
                proxy_set_headers,
            }))
        }
        HandlerConfig::File { root, index, try_files, autoindex } => {
            Ok(RouteAction::File(FileTarget {
                root: super::resolve_path(base_dir, root),
                index: index.and_then(|s| if s.trim().is_empty() { None } else { Some(s) }),
                try_files: try_files.unwrap_or_default(),
                autoindex: autoindex.unwrap_or(false),
            }))
        }
        HandlerConfig::Return { status, location, body } => Ok(RouteAction::Return(ReturnAction {
            status: StatusCode::from_u16(status)?,
            location,
            body,
        })),
        HandlerConfig::Status => Ok(RouteAction::Status),
        HandlerConfig::Metrics => Ok(RouteAction::Metrics),
        HandlerConfig::Config => Ok(RouteAction::Config),
    }
}

fn compile_route_access_control(
    matcher: &RouteMatcher,
    allow_cidrs: Vec<String>,
    deny_cidrs: Vec<String>,
) -> Result<RouteAccessControl> {
    let matcher_label = match matcher {
        RouteMatcher::Exact(path) | RouteMatcher::Prefix(path) => path.as_str(),
    };

    Ok(RouteAccessControl::new(
        compile_cidrs(matcher_label, "allow_cidrs", allow_cidrs)?,
        compile_cidrs(matcher_label, "deny_cidrs", deny_cidrs)?,
    ))
}

fn compile_route_rate_limit(
    matcher: &RouteMatcher,
    requests_per_sec: Option<u32>,
    burst: Option<u32>,
) -> Result<Option<RouteRateLimit>> {
    let matcher_label = match matcher {
        RouteMatcher::Exact(path) | RouteMatcher::Prefix(path) => path.as_str(),
    };

    match requests_per_sec {
        Some(requests_per_sec) => {
            Ok(Some(RouteRateLimit::new(requests_per_sec, burst.unwrap_or(0))))
        }
        None if burst.is_some() => Err(Error::Config(format!(
            "route matcher `{matcher_label}` burst requires requests_per_sec to be set"
        ))),
        None => Ok(None),
    }
}

fn compile_cidrs(route_matcher: &str, field: &str, cidrs: Vec<String>) -> Result<Vec<IpNet>> {
    cidrs
        .into_iter()
        .map(|cidr| {
            let normalized = cidr.trim();
            if normalized.is_empty() {
                return Err(Error::Config(format!(
                    "route matcher `{route_matcher}` {field} entries must not be empty"
                )));
            }

            normalized.parse::<IpNet>().map_err(|error| {
                Error::Config(format!(
                    "route matcher `{route_matcher}` {field} entry `{cidr}` is invalid: {error}"
                ))
            })
        })
        .collect()
}
