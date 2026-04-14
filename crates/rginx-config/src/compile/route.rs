use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use ipnet::IpNet;
use rginx_core::{
    Error, GrpcRouteMatch, ProxyTarget, Result, ReturnAction, Route, RouteAccessControl,
    RouteAction, RouteBufferingPolicy, RouteCompressionPolicy, RouteMatcher, RouteRateLimit,
    Upstream,
};

use crate::model::{
    HandlerConfig, LocationConfig, MatcherConfig, RouteBufferingPolicyConfig,
    RouteCompressionPolicyConfig,
};

pub(super) fn compile_routes(
    locations: Vec<LocationConfig>,
    upstreams: &HashMap<String, Arc<Upstream>>,
    vhost_id: &str,
) -> Result<Vec<Route>> {
    let mut routes = locations
        .into_iter()
        .enumerate()
        .map(|(route_index, location)| compile_route(location, route_index, upstreams, vhost_id))
        .collect::<Result<Vec<_>>>()?;

    routes.sort_by_key(|route| std::cmp::Reverse(route.priority()));

    Ok(routes)
}

fn compile_route(
    location: LocationConfig,
    route_index: usize,
    upstreams: &HashMap<String, Arc<Upstream>>,
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
        allow_early_data,
        request_buffering,
        response_buffering,
        compression,
        compression_min_bytes,
        compression_content_types,
        streaming_response_idle_timeout_secs,
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
    let action = compile_route_action(handler, upstreams)?;

    Ok(Route {
        id: route_id,
        matcher,
        grpc_match,
        action,
        access_control,
        rate_limit,
        allow_early_data: allow_early_data.unwrap_or(false),
        request_buffering: compile_buffering_policy(request_buffering),
        response_buffering: compile_buffering_policy(response_buffering),
        compression: compile_compression_policy(compression),
        compression_min_bytes: compression_min_bytes.map(|value| value as usize),
        compression_content_types: compile_compression_content_types(compression_content_types),
        streaming_response_idle_timeout: streaming_response_idle_timeout_secs
            .map(Duration::from_secs),
    })
}

fn compile_route_action(
    handler: HandlerConfig,
    upstreams: &HashMap<String, Arc<Upstream>>,
) -> Result<RouteAction> {
    match handler {
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
        HandlerConfig::Return { status, location, body } => Ok(RouteAction::Return(ReturnAction {
            status: StatusCode::from_u16(status)?,
            location,
            body,
        })),
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

fn compile_buffering_policy(policy: Option<RouteBufferingPolicyConfig>) -> RouteBufferingPolicy {
    match policy.unwrap_or_default() {
        RouteBufferingPolicyConfig::Auto => RouteBufferingPolicy::Auto,
        RouteBufferingPolicyConfig::On => RouteBufferingPolicy::On,
        RouteBufferingPolicyConfig::Off => RouteBufferingPolicy::Off,
    }
}

fn compile_compression_policy(
    policy: Option<RouteCompressionPolicyConfig>,
) -> RouteCompressionPolicy {
    match policy.unwrap_or_default() {
        RouteCompressionPolicyConfig::Off => RouteCompressionPolicy::Off,
        RouteCompressionPolicyConfig::Auto => RouteCompressionPolicy::Auto,
        RouteCompressionPolicyConfig::Force => RouteCompressionPolicy::Force,
    }
}

fn compile_compression_content_types(content_types: Option<Vec<String>>) -> Vec<String> {
    content_types
        .unwrap_or_default()
        .into_iter()
        .map(|content_type| content_type.trim().to_ascii_lowercase())
        .collect()
}
