use std::collections::HashSet;

use ipnet::IpNet;
use rginx_core::{Error, ProxyHeaderTemplate, Result, RouteRegexMatcher};

use crate::model::{
    HandlerConfig, LocationConfig, MatcherConfig, ProxyHeaderDynamicValueConfig,
    ProxyHeaderValueConfig, RouteBufferingPolicyConfig, RouteCompressionPolicyConfig,
};

pub(super) fn validate_locations(
    scope_label: Option<&str>,
    locations: &[LocationConfig],
    upstream_names: &HashSet<String>,
) -> Result<()> {
    let mut exact_routes = HashSet::new();

    for location in locations {
        let matcher_label = match &location.matcher {
            MatcherConfig::Exact(path) | MatcherConfig::Prefix(path) => {
                if !path.starts_with('/') {
                    return Err(Error::Config(match scope_label {
                        Some(scope_label) => {
                            format!("{scope_label} route matcher `{path}` must start with `/`")
                        }
                        None => format!("route matcher `{path}` must start with `/`"),
                    }));
                }

                path.as_str()
            }
            MatcherConfig::Regex { pattern, case_insensitive } => {
                if pattern.trim().is_empty() {
                    return Err(Error::Config(match scope_label {
                        Some(scope_label) => {
                            format!("{scope_label} route regex matcher must not be empty")
                        }
                        None => "route regex matcher must not be empty".to_string(),
                    }));
                }
                RouteRegexMatcher::new(pattern.clone(), *case_insensitive)
                    .map_err(|error| Error::Config(error.to_string()))?;
                pattern.as_str()
            }
        };

        if let MatcherConfig::Exact(path) = &location.matcher
            && !exact_routes.insert(exact_route_key(
                path,
                location.grpc_service.as_deref(),
                location.grpc_method.as_deref(),
            ))
        {
            return Err(Error::Config(match scope_label {
                Some(scope_label) => format!(
                    "{scope_label} duplicate exact route `{path}` with the same gRPC route constraints"
                ),
                None => {
                    format!("duplicate exact route `{path}` with the same gRPC route constraints")
                }
            }));
        }

        validate_route_cidrs(matcher_label, "allow_cidrs", &location.allow_cidrs)?;
        validate_route_cidrs(matcher_label, "deny_cidrs", &location.deny_cidrs)?;
        validate_route_rate_limit(matcher_label, location.requests_per_sec, location.burst)?;
        validate_route_transport_policy(
            matcher_label,
            location.request_buffering,
            location.response_buffering,
            location.compression,
            location.compression_min_bytes,
            location.compression_content_types.as_deref(),
            location.streaming_response_idle_timeout_secs,
        )?;

        let route_scope = route_scope(scope_label, matcher_label);
        validate_grpc_route_match(
            &route_scope,
            location.grpc_service.as_deref(),
            location.grpc_method.as_deref(),
        )?;
        validate_handler(scope_label, &route_scope, &location.handler, upstream_names)?;
    }

    Ok(())
}

fn validate_handler(
    scope_label: Option<&str>,
    route_scope: &str,
    handler: &HandlerConfig,
    upstream_names: &HashSet<String>,
) -> Result<()> {
    if let HandlerConfig::Proxy { upstream, strip_prefix, proxy_set_headers, .. } = handler {
        if upstream.trim().is_empty() {
            return Err(Error::Config("proxy upstream name must not be empty".to_string()));
        }

        if !upstream_names.contains(upstream) {
            return Err(Error::Config(match scope_label {
                Some(scope_label) => {
                    format!("{scope_label} proxy upstream `{upstream}` is not defined")
                }
                None => format!("proxy upstream `{upstream}` is not defined"),
            }));
        }

        if let Some(prefix) = strip_prefix
            && !prefix.starts_with('/')
        {
            return Err(Error::Config(format!("{route_scope} strip_prefix must start with `/`")));
        }

        for (name, value) in proxy_set_headers {
            if name.trim().is_empty() {
                return Err(Error::Config(format!(
                    "{route_scope} proxy_set_headers name must not be empty"
                )));
            }
            if name.parse::<http::header::HeaderName>().is_err() {
                return Err(Error::Config(format!(
                    "{route_scope} proxy_set_headers name `{name}` is invalid"
                )));
            }
            validate_proxy_header_value(route_scope, name, value)?;
        }
    }

    if let HandlerConfig::Return { status, location, .. } = handler {
        if *status < 100 || *status > 599 {
            return Err(Error::Config(format!(
                "{route_scope} return status must be between 100 and 599"
            )));
        }

        if (300..=399).contains(status) && location.trim().is_empty() {
            return Err(Error::Config(format!(
                "{route_scope} return location must not be empty for redirect status {status}"
            )));
        }
    }
    Ok(())
}

fn validate_proxy_header_value(
    route_scope: &str,
    name: &str,
    value: &ProxyHeaderValueConfig,
) -> Result<()> {
    match value {
        ProxyHeaderValueConfig::Static(value) => {
            value.parse::<http::header::HeaderValue>().map_err(|error| {
                Error::Config(format!(
                    "{route_scope} proxy_set_headers value for `{name}` is invalid: {error}"
                ))
            })?;
        }
        ProxyHeaderValueConfig::Dynamic(dynamic) => match dynamic {
            ProxyHeaderDynamicValueConfig::Host
            | ProxyHeaderDynamicValueConfig::Scheme
            | ProxyHeaderDynamicValueConfig::ClientIp
            | ProxyHeaderDynamicValueConfig::RemoteAddr
            | ProxyHeaderDynamicValueConfig::PeerAddr
            | ProxyHeaderDynamicValueConfig::ForwardedFor => {}
            ProxyHeaderDynamicValueConfig::RequestHeader(header_name) => {
                if header_name.trim().is_empty() {
                    return Err(Error::Config(format!(
                        "{route_scope} proxy_set_headers RequestHeader source for `{name}` must not be empty"
                    )));
                }
                header_name.parse::<http::header::HeaderName>().map_err(|error| {
                    Error::Config(format!(
                        "{route_scope} proxy_set_headers RequestHeader source `{header_name}` for `{name}` is invalid: {error}"
                    ))
                })?;
            }
            ProxyHeaderDynamicValueConfig::Template(template) => {
                ProxyHeaderTemplate::parse(template.clone()).map_err(|error| {
                    Error::Config(format!(
                        "{route_scope} proxy_set_headers Template for `{name}` is invalid: {error}"
                    ))
                })?;
            }
        },
    }

    Ok(())
}

fn validate_route_cidrs(route_matcher: &str, field: &str, cidrs: &[String]) -> Result<()> {
    for cidr in cidrs {
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
        })?;
    }

    Ok(())
}

fn validate_route_rate_limit(
    route_matcher: &str,
    requests_per_sec: Option<u32>,
    burst: Option<u32>,
) -> Result<()> {
    if requests_per_sec.is_some_and(|limit| limit == 0) {
        return Err(Error::Config(format!(
            "route matcher `{route_matcher}` requests_per_sec must be greater than 0"
        )));
    }

    if requests_per_sec.is_none() && burst.is_some() {
        return Err(Error::Config(format!(
            "route matcher `{route_matcher}` burst requires requests_per_sec to be set"
        )));
    }

    Ok(())
}

fn validate_route_transport_policy(
    route_matcher: &str,
    _request_buffering: Option<RouteBufferingPolicyConfig>,
    response_buffering: Option<RouteBufferingPolicyConfig>,
    compression: Option<RouteCompressionPolicyConfig>,
    compression_min_bytes: Option<u64>,
    compression_content_types: Option<&[String]>,
    streaming_response_idle_timeout_secs: Option<u64>,
) -> Result<()> {
    if compression_min_bytes.is_some_and(|value| value == 0) {
        return Err(Error::Config(format!(
            "route matcher `{route_matcher}` compression_min_bytes must be greater than 0"
        )));
    }

    if streaming_response_idle_timeout_secs.is_some_and(|value| value == 0) {
        return Err(Error::Config(format!(
            "route matcher `{route_matcher}` streaming_response_idle_timeout_secs must be greater than 0"
        )));
    }

    if matches!(compression, Some(RouteCompressionPolicyConfig::Force))
        && matches!(response_buffering, Some(RouteBufferingPolicyConfig::Off))
    {
        return Err(Error::Config(format!(
            "route matcher `{route_matcher}` compression=Force requires response_buffering to remain Auto or On"
        )));
    }

    if let Some(content_types) = compression_content_types {
        if content_types.is_empty() {
            return Err(Error::Config(format!(
                "route matcher `{route_matcher}` compression_content_types must not be empty when provided"
            )));
        }

        for content_type in content_types {
            if content_type.trim().is_empty() {
                return Err(Error::Config(format!(
                    "route matcher `{route_matcher}` compression_content_types entries must not be empty"
                )));
            }
        }
    }

    Ok(())
}

fn validate_grpc_route_match(
    route_scope: &str,
    grpc_service: Option<&str>,
    grpc_method: Option<&str>,
) -> Result<()> {
    if let Some(service) = grpc_service {
        if service.trim().is_empty() {
            return Err(Error::Config(format!("{route_scope} grpc_service must not be empty")));
        }
        if service.contains('/') {
            return Err(Error::Config(format!("{route_scope} grpc_service must not contain `/`")));
        }
    }

    if let Some(method) = grpc_method {
        if method.trim().is_empty() {
            return Err(Error::Config(format!("{route_scope} grpc_method must not be empty")));
        }
        if method.contains('/') {
            return Err(Error::Config(format!("{route_scope} grpc_method must not contain `/`")));
        }
    }

    Ok(())
}

fn exact_route_key(path: &str, grpc_service: Option<&str>, grpc_method: Option<&str>) -> String {
    let service = grpc_service.unwrap_or_default();
    let method = grpc_method.unwrap_or_default();
    format!("{path}\0{service}\0{method}")
}

fn route_scope(scope_label: Option<&str>, matcher_label: &str) -> String {
    match scope_label {
        Some(scope_label) => format!("{scope_label} route matcher `{matcher_label}`"),
        None => format!("route matcher `{matcher_label}`"),
    }
}
