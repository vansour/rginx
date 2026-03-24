use std::collections::HashSet;
use std::net::IpAddr;

use http::uri::PathAndQuery;
use ipnet::IpNet;
use rginx_core::{Error, Result};

use crate::model::{
    Config, HandlerConfig, MatcherConfig, ServerTlsConfig, UpstreamProtocolConfig,
    UpstreamTlsConfig, VirtualHostConfig,
};

const DEFAULT_GRPC_HEALTH_CHECK_PATH: &str = "/grpc.health.v1.Health/Check";

pub fn validate(config: &Config) -> Result<()> {
    if config.runtime.shutdown_timeout_secs == 0 {
        return Err(Error::Config(
            "runtime.shutdown_timeout_secs must be greater than 0".to_string(),
        ));
    }

    if config.runtime.worker_threads.is_some_and(|count| count == 0) {
        return Err(Error::Config("runtime.worker_threads must be greater than 0".to_string()));
    }

    if config.runtime.accept_workers.is_some_and(|count| count == 0) {
        return Err(Error::Config("runtime.accept_workers must be greater than 0".to_string()));
    }

    if config.locations.is_empty() {
        return Err(Error::Config("at least one location must be configured".to_string()));
    }

    for value in &config.server.trusted_proxies {
        validate_trusted_proxy(value)?;
    }

    if config.server.max_headers.is_some_and(|limit| limit == 0) {
        return Err(Error::Config("server max_headers must be greater than 0".to_string()));
    }

    if config.server.max_request_body_bytes.is_some_and(|limit| limit == 0) {
        return Err(Error::Config(
            "server max_request_body_bytes must be greater than 0".to_string(),
        ));
    }

    if config.server.max_connections.is_some_and(|limit| limit == 0) {
        return Err(Error::Config("server max_connections must be greater than 0".to_string()));
    }

    if config.server.header_read_timeout_secs.is_some_and(|timeout| timeout == 0) {
        return Err(Error::Config(
            "server header_read_timeout_secs must be greater than 0".to_string(),
        ));
    }

    if config.server.request_body_read_timeout_secs.is_some_and(|timeout| timeout == 0) {
        return Err(Error::Config(
            "server request_body_read_timeout_secs must be greater than 0".to_string(),
        ));
    }

    if config.server.response_write_timeout_secs.is_some_and(|timeout| timeout == 0) {
        return Err(Error::Config(
            "server response_write_timeout_secs must be greater than 0".to_string(),
        ));
    }

    if config.server.access_log_format.as_deref().is_some_and(|format| format.trim().is_empty()) {
        return Err(Error::Config("server access_log_format must not be empty".to_string()));
    }

    if let Some(ServerTlsConfig { cert_path, key_path }) = &config.server.tls {
        if cert_path.trim().is_empty() {
            return Err(Error::Config("server TLS certificate path must not be empty".to_string()));
        }

        if key_path.trim().is_empty() {
            return Err(Error::Config("server TLS private key path must not be empty".to_string()));
        }
    }

    let mut upstream_names = HashSet::new();
    for upstream in &config.upstreams {
        if upstream.name.trim().is_empty() {
            return Err(Error::Config("upstream name must not be empty".to_string()));
        }

        if !upstream_names.insert(upstream.name.clone()) {
            return Err(Error::Config(format!("duplicate upstream `{}`", upstream.name)));
        }

        if upstream.peers.is_empty() {
            return Err(Error::Config(format!(
                "upstream `{}` must define at least one peer",
                upstream.name
            )));
        }

        for peer in &upstream.peers {
            if peer.url.trim().is_empty() {
                return Err(Error::Config(format!(
                    "upstream `{}` contains an empty peer url",
                    upstream.name
                )));
            }

            if peer.weight == 0 {
                return Err(Error::Config(format!(
                    "upstream `{}` peer `{}` weight must be greater than 0",
                    upstream.name, peer.url
                )));
            }
        }

        if let Some(UpstreamTlsConfig::CustomCa { ca_cert_path }) = &upstream.tls
            && ca_cert_path.trim().is_empty()
        {
            return Err(Error::Config(format!(
                "upstream `{}` custom CA path must not be empty",
                upstream.name
            )));
        }

        if let Some(server_name_override) = &upstream.server_name_override
            && server_name_override.trim().is_empty()
        {
            return Err(Error::Config(format!(
                "upstream `{}` server_name_override must not be empty",
                upstream.name
            )));
        }

        if matches!(upstream.protocol, UpstreamProtocolConfig::Http2) {
            for peer in &upstream.peers {
                let Ok(uri) = peer.url.parse::<http::Uri>() else {
                    continue;
                };

                if uri.scheme_str() != Some("https") {
                    return Err(Error::Config(format!(
                        "upstream `{}` protocol `Http2` currently requires all peers to use `https://`; cleartext h2c upstreams are not supported",
                        upstream.name
                    )));
                }
            }
        }

        if upstream.request_timeout_secs.is_some_and(|timeout| timeout == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` request_timeout_secs must be greater than 0",
                upstream.name
            )));
        }

        if upstream.connect_timeout_secs.is_some_and(|timeout| timeout == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` connect_timeout_secs must be greater than 0",
                upstream.name
            )));
        }

        if upstream.read_timeout_secs.is_some_and(|timeout| timeout == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` read_timeout_secs must be greater than 0",
                upstream.name
            )));
        }

        if upstream.write_timeout_secs.is_some_and(|timeout| timeout == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` write_timeout_secs must be greater than 0",
                upstream.name
            )));
        }

        if upstream.idle_timeout_secs.is_some_and(|timeout| timeout == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` idle_timeout_secs must be greater than 0",
                upstream.name
            )));
        }

        if upstream.tcp_keepalive_secs.is_some_and(|timeout| timeout == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` tcp_keepalive_secs must be greater than 0",
                upstream.name
            )));
        }

        if upstream.http2_keep_alive_interval_secs.is_some_and(|timeout| timeout == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` http2_keep_alive_interval_secs must be greater than 0",
                upstream.name
            )));
        }

        if upstream.http2_keep_alive_timeout_secs.is_some_and(|timeout| timeout == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` http2_keep_alive_timeout_secs must be greater than 0",
                upstream.name
            )));
        }

        let has_http2_keep_alive_tuning = upstream.http2_keep_alive_timeout_secs.is_some()
            || upstream.http2_keep_alive_while_idle.is_some();
        if upstream.http2_keep_alive_interval_secs.is_none() && has_http2_keep_alive_tuning {
            return Err(Error::Config(format!(
                "upstream `{}` http2_keep_alive_timeout_secs and http2_keep_alive_while_idle require http2_keep_alive_interval_secs to be set",
                upstream.name
            )));
        }

        if upstream.max_replayable_request_body_bytes.is_some_and(|bytes| bytes == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` max_replayable_request_body_bytes must be greater than 0",
                upstream.name
            )));
        }

        if upstream.unhealthy_after_failures.is_some_and(|failures| failures == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` unhealthy_after_failures must be greater than 0",
                upstream.name
            )));
        }

        if upstream.unhealthy_cooldown_secs.is_some_and(|cooldown| cooldown == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` unhealthy_cooldown_secs must be greater than 0",
                upstream.name
            )));
        }

        if let Some(path) = &upstream.health_check_path {
            if path.trim().is_empty() {
                return Err(Error::Config(format!(
                    "upstream `{}` health_check_path must not be empty",
                    upstream.name
                )));
            }

            if !path.starts_with('/') {
                return Err(Error::Config(format!(
                    "upstream `{}` health_check_path must start with `/`",
                    upstream.name
                )));
            }

            PathAndQuery::from_maybe_shared(path.clone()).map_err(|error| {
                Error::Config(format!(
                    "upstream `{}` health_check_path `{path}` is invalid: {error}",
                    upstream.name
                ))
            })?;
        }

        if let Some(service) = &upstream.health_check_grpc_service {
            if !service.is_empty() && service.trim().is_empty() {
                return Err(Error::Config(format!(
                    "upstream `{}` health_check_grpc_service must not be blank",
                    upstream.name
                )));
            }

            if service.contains('/') {
                return Err(Error::Config(format!(
                    "upstream `{}` health_check_grpc_service must not contain `/`",
                    upstream.name
                )));
            }

            if let Some(path) = &upstream.health_check_path
                && path != DEFAULT_GRPC_HEALTH_CHECK_PATH
            {
                return Err(Error::Config(format!(
                    "upstream `{}` health_check_grpc_service requires health_check_path to be `{DEFAULT_GRPC_HEALTH_CHECK_PATH}`",
                    upstream.name
                )));
            }

            if matches!(upstream.protocol, UpstreamProtocolConfig::Http1) {
                return Err(Error::Config(format!(
                    "upstream `{}` health_check_grpc_service requires protocol `Auto` or `Http2`",
                    upstream.name
                )));
            }

            if upstream.peers.iter().any(|peer| !peer.url.starts_with("https://")) {
                return Err(Error::Config(format!(
                    "upstream `{}` health_check_grpc_service currently requires all peers to use `https://`; cleartext h2c health checks are not supported",
                    upstream.name
                )));
            }
        }

        let has_active_health_overrides = upstream.health_check_interval_secs.is_some()
            || upstream.health_check_timeout_secs.is_some()
            || upstream.healthy_successes_required.is_some();
        if upstream.health_check_path.is_none()
            && upstream.health_check_grpc_service.is_none()
            && has_active_health_overrides
        {
            return Err(Error::Config(format!(
                "upstream `{}` active health-check tuning requires health_check_path or health_check_grpc_service to be set",
                upstream.name
            )));
        }

        if upstream.health_check_interval_secs.is_some_and(|interval| interval == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` health_check_interval_secs must be greater than 0",
                upstream.name
            )));
        }

        if upstream.health_check_timeout_secs.is_some_and(|timeout| timeout == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` health_check_timeout_secs must be greater than 0",
                upstream.name
            )));
        }

        if upstream.healthy_successes_required.is_some_and(|successes| successes == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` healthy_successes_required must be greater than 0",
                upstream.name
            )));
        }
    }

    let mut exact_routes = HashSet::new();

    for location in &config.locations {
        let matcher_label = match &location.matcher {
            MatcherConfig::Exact(path) | MatcherConfig::Prefix(path) => {
                if !path.starts_with('/') {
                    return Err(Error::Config(format!(
                        "route matcher `{path}` must start with `/`"
                    )));
                }

                path.as_str()
            }
        };

        if let MatcherConfig::Exact(path) = &location.matcher
            && !exact_routes.insert(exact_route_key(
                path,
                location.grpc_service.as_deref(),
                location.grpc_method.as_deref(),
            ))
        {
            return Err(Error::Config(format!(
                "duplicate exact route `{path}` with the same gRPC route constraints"
            )));
        }

        validate_route_cidrs(matcher_label, "allow_cidrs", &location.allow_cidrs)?;
        validate_route_cidrs(matcher_label, "deny_cidrs", &location.deny_cidrs)?;
        validate_route_rate_limit(matcher_label, location.requests_per_sec, location.burst)?;
        validate_grpc_route_match(
            &format!("route matcher `{matcher_label}`"),
            location.grpc_service.as_deref(),
            location.grpc_method.as_deref(),
        )?;
        validate_management_handler_constraints(
            &format!("route matcher `{matcher_label}`"),
            &location.matcher,
            &location.handler,
            &location.allow_cidrs,
        )?;

        if let HandlerConfig::Proxy { upstream, strip_prefix, proxy_set_headers, .. } =
            &location.handler
        {
            if upstream.trim().is_empty() {
                return Err(Error::Config("proxy upstream name must not be empty".to_string()));
            }

            if !upstream_names.contains(upstream) {
                return Err(Error::Config(format!("proxy upstream `{upstream}` is not defined")));
            }

            if let Some(prefix) = strip_prefix
                && !prefix.starts_with('/')
            {
                return Err(Error::Config(format!(
                    "route matcher `{matcher_label}` strip_prefix must start with `/`"
                )));
            }

            for name in proxy_set_headers.keys() {
                if name.trim().is_empty() {
                    return Err(Error::Config(format!(
                        "route matcher `{matcher_label}` proxy_set_headers name must not be empty"
                    )));
                }
                if name.parse::<http::header::HeaderName>().is_err() {
                    return Err(Error::Config(format!(
                        "route matcher `{matcher_label}` proxy_set_headers name `{name}` is invalid"
                    )));
                }
            }
        }

        if let HandlerConfig::File { root, index, try_files, autoindex: _ } = &location.handler {
            if root.trim().is_empty() {
                return Err(Error::Config(format!(
                    "route matcher `{matcher_label}` file root must not be empty"
                )));
            }

            if let Some(index) = index
                && index.trim().is_empty()
            {
                return Err(Error::Config(format!(
                    "route matcher `{matcher_label}` file index must not be empty"
                )));
            }

            if let Some(try_files) = try_files {
                if try_files.is_empty() {
                    return Err(Error::Config(format!(
                        "route matcher `{matcher_label}` try_files must not be empty"
                    )));
                }
                for candidate in try_files {
                    if candidate.trim().is_empty() {
                        return Err(Error::Config(format!(
                            "route matcher `{matcher_label}` try_files entries must not be empty"
                        )));
                    }
                }
            }
        }

        if let HandlerConfig::Return { status, location, .. } = &location.handler {
            if *status < 100 || *status > 599 {
                return Err(Error::Config(format!(
                    "route matcher `{matcher_label}` return status must be between 100 and 599"
                )));
            }

            // For 3xx redirects, location should not be empty
            if (300..=399).contains(status) && location.trim().is_empty() {
                return Err(Error::Config(format!(
                    "route matcher `{matcher_label}` return location must not be empty for redirect status {status}"
                )));
            }
        }
    }

    let mut all_server_names = HashSet::new();
    validate_server_names("server", &config.server.server_names, &mut all_server_names)?;
    validate_virtual_hosts(&config.servers, &upstream_names, &mut all_server_names)?;

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

fn validate_management_handler_constraints(
    route_scope: &str,
    matcher: &MatcherConfig,
    handler: &HandlerConfig,
    allow_cidrs: &[String],
) -> Result<()> {
    if !matches!(handler, HandlerConfig::Config) {
        return Ok(());
    }

    if !matches!(matcher, MatcherConfig::Exact(_)) {
        return Err(Error::Config(format!(
            "{route_scope} config handler requires an Exact(...) matcher"
        )));
    }

    if allow_cidrs.is_empty() {
        return Err(Error::Config(format!(
            "{route_scope} config handler requires non-empty allow_cidrs"
        )));
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

fn validate_trusted_proxy(value: &str) -> Result<()> {
    let normalized = normalize_trusted_proxy(value).ok_or_else(|| {
        Error::Config(format!(
            "server trusted_proxies entry `{value}` must be a valid IP address or CIDR"
        ))
    })?;

    normalized.parse::<IpNet>().map_err(|error| {
        Error::Config(format!("server trusted_proxies entry `{value}` is invalid: {error}"))
    })?;

    Ok(())
}

fn normalize_trusted_proxy(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.contains('/') {
        return Some(trimmed.to_string());
    }

    let ip = trimmed.parse::<IpAddr>().ok()?;
    Some(match ip {
        IpAddr::V4(_) => format!("{trimmed}/32"),
        IpAddr::V6(_) => format!("{trimmed}/128"),
    })
}

fn validate_server_names(
    owner_label: &str,
    server_names: &[String],
    all_server_names: &mut HashSet<String>,
) -> Result<()> {
    for name in server_names {
        let normalized = name.trim().to_lowercase();
        if normalized.is_empty() {
            return Err(Error::Config(format!("{owner_label} server_name must not be empty")));
        }

        if normalized.contains('/') {
            return Err(Error::Config(format!(
                "{owner_label} server_name `{name}` should not contain path separator"
            )));
        }

        if !all_server_names.insert(normalized) {
            return Err(Error::Config(format!(
                "duplicate server_name `{name}` across server and servers"
            )));
        }
    }

    Ok(())
}

fn validate_virtual_hosts(
    vhosts: &[VirtualHostConfig],
    upstream_names: &HashSet<String>,
    all_server_names: &mut HashSet<String>,
) -> Result<()> {
    for (idx, vhost) in vhosts.iter().enumerate() {
        let vhost_label = format!("servers[{idx}]");

        if vhost.server_names.is_empty() {
            if vhost.tls.is_some() {
                return Err(Error::Config(format!(
                    "{vhost_label} TLS requires at least one server_name"
                )));
            }

            return Err(Error::Config(format!(
                "{vhost_label} must define at least one server_name"
            )));
        }

        validate_server_names(&vhost_label, &vhost.server_names, all_server_names)?;

        if vhost.locations.is_empty() {
            return Err(Error::Config(format!("{vhost_label} must have at least one location")));
        }

        if let Some(tls) = &vhost.tls {
            if tls.cert_path.trim().is_empty() {
                return Err(Error::Config(format!(
                    "{vhost_label} TLS certificate path must not be empty"
                )));
            }

            if tls.key_path.trim().is_empty() {
                return Err(Error::Config(format!(
                    "{vhost_label} TLS private key path must not be empty"
                )));
            }
        }

        let mut vhost_exact_routes = HashSet::new();
        for location in &vhost.locations {
            let matcher_label = match &location.matcher {
                MatcherConfig::Exact(path) | MatcherConfig::Prefix(path) => {
                    if !path.starts_with('/') {
                        return Err(Error::Config(format!(
                            "{vhost_label} route matcher `{path}` must start with `/`"
                        )));
                    }
                    path.as_str()
                }
            };

            if let MatcherConfig::Exact(path) = &location.matcher
                && !vhost_exact_routes.insert(exact_route_key(
                    path,
                    location.grpc_service.as_deref(),
                    location.grpc_method.as_deref(),
                ))
            {
                return Err(Error::Config(format!(
                    "{vhost_label} duplicate exact route `{path}` with the same gRPC route constraints"
                )));
            }

            validate_route_cidrs(matcher_label, "allow_cidrs", &location.allow_cidrs)?;
            validate_route_cidrs(matcher_label, "deny_cidrs", &location.deny_cidrs)?;
            validate_route_rate_limit(matcher_label, location.requests_per_sec, location.burst)?;
            validate_grpc_route_match(
                &format!("{vhost_label} route matcher `{matcher_label}`"),
                location.grpc_service.as_deref(),
                location.grpc_method.as_deref(),
            )?;
            validate_management_handler_constraints(
                &format!("{vhost_label} route matcher `{matcher_label}`"),
                &location.matcher,
                &location.handler,
                &location.allow_cidrs,
            )?;

            if let HandlerConfig::Proxy { upstream, strip_prefix, proxy_set_headers, .. } =
                &location.handler
            {
                if upstream.trim().is_empty() {
                    return Err(Error::Config("proxy upstream name must not be empty".to_string()));
                }

                if !upstream_names.contains(upstream) {
                    return Err(Error::Config(format!(
                        "{vhost_label} proxy upstream `{upstream}` is not defined"
                    )));
                }

                if let Some(prefix) = strip_prefix
                    && !prefix.starts_with('/')
                {
                    return Err(Error::Config(format!(
                        "{vhost_label} route matcher `{matcher_label}` strip_prefix must start with `/`"
                    )));
                }

                for name in proxy_set_headers.keys() {
                    if name.trim().is_empty() {
                        return Err(Error::Config(format!(
                            "{vhost_label} route matcher `{matcher_label}` proxy_set_headers name must not be empty"
                        )));
                    }
                    if name.parse::<http::header::HeaderName>().is_err() {
                        return Err(Error::Config(format!(
                            "{vhost_label} route matcher `{matcher_label}` proxy_set_headers name `{name}` is invalid"
                        )));
                    }
                }
            }

            if let HandlerConfig::File { root, index, try_files, autoindex: _ } = &location.handler
            {
                if root.trim().is_empty() {
                    return Err(Error::Config(format!(
                        "{vhost_label} route matcher `{matcher_label}` file root must not be empty"
                    )));
                }

                if let Some(index) = index
                    && index.trim().is_empty()
                {
                    return Err(Error::Config(format!(
                        "{vhost_label} route matcher `{matcher_label}` file index must not be empty"
                    )));
                }

                if let Some(try_files) = try_files {
                    if try_files.is_empty() {
                        return Err(Error::Config(format!(
                            "{vhost_label} route matcher `{matcher_label}` try_files must not be empty"
                        )));
                    }
                    for candidate in try_files {
                        if candidate.trim().is_empty() {
                            return Err(Error::Config(format!(
                                "{vhost_label} route matcher `{matcher_label}` try_files entries must not be empty"
                            )));
                        }
                    }
                }
            }

            if let HandlerConfig::Return { status, location, .. } = &location.handler {
                if *status < 100 || *status > 599 {
                    return Err(Error::Config(format!(
                        "{vhost_label} route matcher `{matcher_label}` return status must be between 100 and 599"
                    )));
                }

                // For 3xx redirects, location should not be empty
                if (300..=399).contains(status) && location.trim().is_empty() {
                    return Err(Error::Config(format!(
                        "{vhost_label} route matcher `{matcher_label}` return location must not be empty for redirect status {status}"
                    )));
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::model::{
        Config, HandlerConfig, LocationConfig, MatcherConfig, RuntimeConfig, ServerConfig,
        ServerTlsConfig, UpstreamConfig, UpstreamLoadBalanceConfig, UpstreamPeerConfig,
        UpstreamProtocolConfig, VirtualHostConfig,
    };

    use super::validate;

    #[test]
    fn validate_rejects_zero_max_replayable_body_size() {
        let mut config = base_config();
        config.upstreams[0].max_replayable_request_body_bytes = Some(0);

        let error = validate(&config).expect_err("zero body size should be rejected");
        assert!(
            error.to_string().contains("max_replayable_request_body_bytes must be greater than 0")
        );
    }

    #[test]
    fn validate_rejects_zero_unhealthy_after_failures() {
        let mut config = base_config();
        config.upstreams[0].unhealthy_after_failures = Some(0);

        let error = validate(&config).expect_err("zero failure threshold should be rejected");
        assert!(error.to_string().contains("unhealthy_after_failures must be greater than 0"));
    }

    #[test]
    fn validate_rejects_zero_unhealthy_cooldown() {
        let mut config = base_config();
        config.upstreams[0].unhealthy_cooldown_secs = Some(0);

        let error = validate(&config).expect_err("zero cooldown should be rejected");
        assert!(error.to_string().contains("unhealthy_cooldown_secs must be greater than 0"));
    }

    #[test]
    fn validate_rejects_zero_connect_timeout() {
        let mut config = base_config();
        config.upstreams[0].connect_timeout_secs = Some(0);

        let error = validate(&config).expect_err("zero connect timeout should be rejected");
        assert!(error.to_string().contains("connect_timeout_secs must be greater than 0"));
    }

    #[test]
    fn validate_rejects_zero_write_timeout() {
        let mut config = base_config();
        config.upstreams[0].write_timeout_secs = Some(0);

        let error = validate(&config).expect_err("zero write timeout should be rejected");
        assert!(error.to_string().contains("write_timeout_secs must be greater than 0"));
    }

    #[test]
    fn validate_rejects_zero_idle_timeout() {
        let mut config = base_config();
        config.upstreams[0].idle_timeout_secs = Some(0);

        let error = validate(&config).expect_err("zero idle timeout should be rejected");
        assert!(error.to_string().contains("idle_timeout_secs must be greater than 0"));
    }

    #[test]
    fn validate_allows_disabling_pool_idle_timeout() {
        let mut config = base_config();
        config.upstreams[0].pool_idle_timeout_secs = Some(0);

        validate(&config).expect("pool_idle_timeout_secs: Some(0) should be accepted");
    }

    #[test]
    fn validate_rejects_zero_tcp_keepalive_timeout() {
        let mut config = base_config();
        config.upstreams[0].tcp_keepalive_secs = Some(0);

        let error = validate(&config).expect_err("zero tcp keepalive should be rejected");
        assert!(error.to_string().contains("tcp_keepalive_secs must be greater than 0"));
    }

    #[test]
    fn validate_rejects_http2_keepalive_tuning_without_interval() {
        let mut config = base_config();
        config.upstreams[0].http2_keep_alive_timeout_secs = Some(5);

        let error =
            validate(&config).expect_err("http2 keepalive tuning should require an interval");
        assert!(error.to_string().contains(
            "http2_keep_alive_timeout_secs and http2_keep_alive_while_idle require http2_keep_alive_interval_secs to be set"
        ));
    }

    #[test]
    fn validate_rejects_zero_http2_keepalive_interval() {
        let mut config = base_config();
        config.upstreams[0].http2_keep_alive_interval_secs = Some(0);

        let error =
            validate(&config).expect_err("zero http2 keepalive interval should be rejected");
        assert!(
            error.to_string().contains("http2_keep_alive_interval_secs must be greater than 0")
        );
    }

    #[test]
    fn validate_rejects_invalid_route_allow_cidr() {
        let mut config = base_config();
        config.locations[0].allow_cidrs = vec!["not-a-cidr".to_string()];

        let error = validate(&config).expect_err("invalid CIDR should be rejected");
        assert!(error.to_string().contains("allow_cidrs entry `not-a-cidr` is invalid"));
    }

    #[test]
    fn validate_rejects_invalid_trusted_proxy() {
        let mut config = base_config();
        config.server.trusted_proxies = vec!["bad-proxy".to_string()];

        let error = validate(&config).expect_err("invalid trusted proxy should be rejected");
        assert!(error.to_string().contains("server trusted_proxies entry `bad-proxy`"));
    }

    #[test]
    fn validate_rejects_empty_server_tls_cert_path() {
        let mut config = base_config();
        config.server.tls = Some(ServerTlsConfig {
            cert_path: " ".to_string(),
            key_path: "server.key".to_string(),
        });

        let error = validate(&config).expect_err("empty cert path should be rejected");
        assert!(error.to_string().contains("server TLS certificate path must not be empty"));
    }

    #[test]
    fn validate_rejects_zero_route_requests_per_sec() {
        let mut config = base_config();
        config.locations[0].requests_per_sec = Some(0);

        let error = validate(&config).expect_err("zero requests_per_sec should be rejected");
        assert!(error.to_string().contains("requests_per_sec must be greater than 0"));
    }

    #[test]
    fn validate_rejects_burst_without_rate_limit() {
        let mut config = base_config();
        config.locations[0].burst = Some(2);

        let error = validate(&config).expect_err("burst without rate limit should be rejected");
        assert!(error.to_string().contains("burst requires requests_per_sec to be set"));
    }

    #[test]
    fn validate_rejects_zero_runtime_worker_threads() {
        let mut config = base_config();
        config.runtime.worker_threads = Some(0);

        let error = validate(&config).expect_err("zero runtime worker threads should be rejected");
        assert!(error.to_string().contains("runtime.worker_threads must be greater than 0"));
    }

    #[test]
    fn validate_rejects_zero_runtime_accept_workers() {
        let mut config = base_config();
        config.runtime.accept_workers = Some(0);

        let error = validate(&config).expect_err("zero runtime accept workers should be rejected");
        assert!(error.to_string().contains("runtime.accept_workers must be greater than 0"));
    }

    #[test]
    fn validate_rejects_zero_server_max_connections() {
        let mut config = base_config();
        config.server.max_connections = Some(0);

        let error = validate(&config).expect_err("zero max connections should be rejected");
        assert!(error.to_string().contains("server max_connections must be greater than 0"));
    }

    #[test]
    fn validate_rejects_zero_server_header_read_timeout() {
        let mut config = base_config();
        config.server.header_read_timeout_secs = Some(0);

        let error = validate(&config).expect_err("zero header timeout should be rejected");
        assert!(
            error.to_string().contains("server header_read_timeout_secs must be greater than 0")
        );
    }

    #[test]
    fn validate_rejects_zero_server_request_body_read_timeout() {
        let mut config = base_config();
        config.server.request_body_read_timeout_secs = Some(0);

        let error =
            validate(&config).expect_err("zero request body read timeout should be rejected");
        assert!(
            error
                .to_string()
                .contains("server request_body_read_timeout_secs must be greater than 0")
        );
    }

    #[test]
    fn validate_rejects_zero_server_response_write_timeout() {
        let mut config = base_config();
        config.server.response_write_timeout_secs = Some(0);

        let error = validate(&config).expect_err("zero response write timeout should be rejected");
        assert!(
            error.to_string().contains("server response_write_timeout_secs must be greater than 0")
        );
    }

    #[test]
    fn validate_rejects_empty_server_access_log_format() {
        let mut config = base_config();
        config.server.access_log_format = Some("   ".to_string());

        let error = validate(&config).expect_err("empty access log format should be rejected");
        assert!(error.to_string().contains("server access_log_format must not be empty"));
    }

    #[test]
    fn validate_rejects_zero_server_max_headers() {
        let mut config = base_config();
        config.server.max_headers = Some(0);

        let error = validate(&config).expect_err("zero max headers should be rejected");
        assert!(error.to_string().contains("server max_headers must be greater than 0"));
    }

    #[test]
    fn validate_rejects_zero_server_max_request_body_bytes() {
        let mut config = base_config();
        config.server.max_request_body_bytes = Some(0);

        let error = validate(&config).expect_err("zero max request body should be rejected");
        assert!(error.to_string().contains("server max_request_body_bytes must be greater than 0"));
    }

    #[test]
    fn validate_rejects_empty_default_server_name() {
        let mut config = base_config();
        config.server.server_names = vec![" ".to_string()];

        let error = validate(&config).expect_err("empty default server_name should be rejected");
        assert!(error.to_string().contains("server server_name must not be empty"));
    }

    #[test]
    fn validate_rejects_default_server_name_with_path_separator() {
        let mut config = base_config();
        config.server.server_names = vec!["api/example.com".to_string()];

        let error = validate(&config).expect_err("invalid default server_name should be rejected");
        assert!(
            error
                .to_string()
                .contains("server server_name `api/example.com` should not contain path separator")
        );
    }

    #[test]
    fn validate_rejects_duplicate_server_name_between_default_server_and_vhost() {
        let mut config = base_config();
        config.server.server_names = vec!["api.example.com".to_string()];
        config.servers = vec![sample_vhost(vec!["API.EXAMPLE.COM"])];

        let error = validate(&config).expect_err("duplicate server_names should be rejected");
        assert!(
            error
                .to_string()
                .contains("duplicate server_name `API.EXAMPLE.COM` across server and servers")
        );
    }

    #[test]
    fn validate_rejects_vhost_without_server_name() {
        let mut config = base_config();
        config.servers = vec![sample_vhost(Vec::new())];

        let error = validate(&config).expect_err("vhost without server_name should be rejected");
        assert!(error.to_string().contains("servers[0] must define at least one server_name"));
    }

    #[test]
    fn validate_rejects_tls_vhost_without_server_name() {
        let mut config = base_config();
        let mut vhost = sample_vhost(Vec::new());
        vhost.tls = Some(ServerTlsConfig {
            cert_path: "server.crt".to_string(),
            key_path: "server.key".to_string(),
        });
        config.servers = vec![vhost];

        let error =
            validate(&config).expect_err("TLS vhost without server_name should be rejected");
        assert!(error.to_string().contains("servers[0] TLS requires at least one server_name"));
    }

    #[test]
    fn validate_rejects_config_handler_without_allow_cidrs() {
        let mut config = base_config();
        config.locations[0].matcher = MatcherConfig::Exact("/-/config".to_string());
        config.locations[0].handler = HandlerConfig::Config;

        let error = validate(&config).expect_err("config handler should require allow_cidrs");
        assert!(error.to_string().contains("config handler requires non-empty allow_cidrs"));
    }

    #[test]
    fn validate_rejects_config_handler_with_prefix_matcher() {
        let mut config = base_config();
        config.locations[0].matcher = MatcherConfig::Prefix("/-/config".to_string());
        config.locations[0].handler = HandlerConfig::Config;
        config.locations[0].allow_cidrs = vec!["127.0.0.1/32".to_string()];

        let error = validate(&config).expect_err("config handler should require exact matcher");
        assert!(error.to_string().contains("config handler requires an Exact(...) matcher"));
    }

    #[test]
    fn validate_rejects_empty_grpc_service() {
        let mut config = base_config();
        config.locations[0].grpc_service = Some("   ".to_string());

        let error = validate(&config).expect_err("empty grpc_service should be rejected");
        assert!(error.to_string().contains("grpc_service must not be empty"));
    }

    #[test]
    fn validate_allows_duplicate_exact_routes_when_grpc_constraints_differ() {
        let mut config = base_config();
        config.locations[0].matcher =
            MatcherConfig::Exact("/grpc.health.v1.Health/Check".to_string());
        config.locations.push(LocationConfig {
            matcher: MatcherConfig::Exact("/grpc.health.v1.Health/Check".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: Some("grpc.health.v1.Health".to_string()),
            grpc_method: Some("Check".to_string()),
            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        });

        validate(&config).expect("different gRPC route constraints should be allowed");
    }

    #[test]
    fn validate_rejects_duplicate_exact_routes_when_grpc_constraints_match() {
        let mut config = base_config();
        config.locations[0].matcher =
            MatcherConfig::Exact("/grpc.health.v1.Health/Check".to_string());
        config.locations[0].grpc_service = Some("grpc.health.v1.Health".to_string());
        config.locations[0].grpc_method = Some("Check".to_string());
        config.locations.push(config.locations[0].clone());

        let error =
            validate(&config).expect_err("duplicate exact route with same gRPC match should fail");
        assert!(
            error
                .to_string()
                .contains("duplicate exact route `/grpc.health.v1.Health/Check` with the same gRPC route constraints")
        );
    }

    fn base_config() -> Config {
        Config {
            runtime: RuntimeConfig {
                shutdown_timeout_secs: 10,
                worker_threads: None,
                accept_workers: None,
            },
            server: ServerConfig {
                listen: "127.0.0.1:8080".to_string(),
                server_names: Vec::new(),
                trusted_proxies: Vec::new(),
                keep_alive: None,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout_secs: None,
                request_body_read_timeout_secs: None,
                response_write_timeout_secs: None,
                access_log_format: None,
                tls: None,
            },
            upstreams: vec![UpstreamConfig {
                name: "backend".to_string(),
                peers: vec![UpstreamPeerConfig {
                    url: "http://127.0.0.1:9000".to_string(),
                    weight: 1,
                    backup: false,
                }],
                tls: None,
                protocol: UpstreamProtocolConfig::Auto,
                load_balance: UpstreamLoadBalanceConfig::RoundRobin,
                server_name_override: None,
                request_timeout_secs: None,
                connect_timeout_secs: None,
                read_timeout_secs: None,
                write_timeout_secs: None,
                idle_timeout_secs: None,
                pool_idle_timeout_secs: None,
                pool_max_idle_per_host: None,
                tcp_keepalive_secs: None,
                tcp_nodelay: None,
                http2_keep_alive_interval_secs: None,
                http2_keep_alive_timeout_secs: None,
                http2_keep_alive_while_idle: None,
                max_replayable_request_body_bytes: None,
                unhealthy_after_failures: None,
                unhealthy_cooldown_secs: None,
                health_check_path: None,
                health_check_grpc_service: None,
                health_check_interval_secs: None,
                health_check_timeout_secs: None,
                healthy_successes_required: None,
            }],
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Prefix("/".to_string()),
                handler: HandlerConfig::Proxy {
                    upstream: "backend".to_string(),
                    preserve_host: None,
                    strip_prefix: None,
                    proxy_set_headers: std::collections::HashMap::new(),
                },
                grpc_service: None,

                grpc_method: None,

                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: None,
                burst: None,
            }],
            servers: Vec::new(),
        }
    }

    #[test]
    fn validate_rejects_active_health_tuning_without_path() {
        let mut config = base_config();
        config.upstreams[0].health_check_timeout_secs = Some(1);

        let error = validate(&config).expect_err("active health tuning should require a path");
        assert!(
            error.to_string().contains("active health-check tuning requires health_check_path")
        );
    }

    #[test]
    fn validate_allows_grpc_health_check_with_default_path() {
        let mut config = base_config();
        config.upstreams[0].peers[0].url = "https://example.com".to_string();
        config.upstreams[0].health_check_grpc_service = Some("grpc.health.v1.Health".to_string());

        validate(&config).expect("gRPC health-check config should validate");
    }

    #[test]
    fn validate_rejects_grpc_health_check_with_non_default_path() {
        let mut config = base_config();
        config.upstreams[0].peers[0].url = "https://example.com".to_string();
        config.upstreams[0].health_check_path = Some("/custom".to_string());
        config.upstreams[0].health_check_grpc_service = Some("grpc.health.v1.Health".to_string());

        let error =
            validate(&config).expect_err("custom gRPC health-check path should be rejected");
        assert!(error.to_string().contains(super::DEFAULT_GRPC_HEALTH_CHECK_PATH));
    }

    #[test]
    fn validate_rejects_grpc_health_check_for_http1_upstream() {
        let mut config = base_config();
        config.upstreams[0].peers[0].url = "https://example.com".to_string();
        config.upstreams[0].protocol = UpstreamProtocolConfig::Http1;
        config.upstreams[0].health_check_grpc_service = Some("grpc.health.v1.Health".to_string());

        let error = validate(&config).expect_err("http1 gRPC health-check should be rejected");
        assert!(error.to_string().contains("requires protocol `Auto` or `Http2`"));
    }

    #[test]
    fn validate_rejects_grpc_health_check_for_cleartext_peer() {
        let mut config = base_config();
        config.upstreams[0].health_check_grpc_service = Some("grpc.health.v1.Health".to_string());

        let error =
            validate(&config).expect_err("cleartext gRPC health-check peer should be rejected");
        assert!(error.to_string().contains("cleartext h2c health checks are not supported"));
    }

    #[test]
    fn validate_rejects_invalid_health_check_path() {
        let mut config = base_config();
        config.upstreams[0].health_check_path = Some("healthz".to_string());

        let error = validate(&config).expect_err("invalid health check path should be rejected");
        assert!(error.to_string().contains("health_check_path must start with `/`"));
    }

    #[test]
    fn validate_rejects_zero_health_check_interval() {
        let mut config = base_config();
        config.upstreams[0].health_check_path = Some("/healthz".to_string());
        config.upstreams[0].health_check_interval_secs = Some(0);

        let error = validate(&config).expect_err("zero health check interval should be rejected");
        assert!(error.to_string().contains("health_check_interval_secs must be greater than 0"));
    }

    #[test]
    fn validate_rejects_zero_health_check_timeout() {
        let mut config = base_config();
        config.upstreams[0].health_check_path = Some("/healthz".to_string());
        config.upstreams[0].health_check_timeout_secs = Some(0);

        let error = validate(&config).expect_err("zero health check timeout should be rejected");
        assert!(error.to_string().contains("health_check_timeout_secs must be greater than 0"));
    }

    #[test]
    fn validate_rejects_zero_peer_weight() {
        let mut config = base_config();
        config.upstreams[0].peers[0].weight = 0;

        let error = validate(&config).expect_err("zero peer weight should be rejected");
        assert!(error.to_string().contains("weight must be greater than 0"));
    }

    #[test]
    fn validate_rejects_zero_healthy_successes_required() {
        let mut config = base_config();
        config.upstreams[0].health_check_path = Some("/healthz".to_string());
        config.upstreams[0].healthy_successes_required = Some(0);

        let error = validate(&config).expect_err("zero recovery threshold should be rejected");
        assert!(error.to_string().contains("healthy_successes_required must be greater than 0"));
    }

    #[test]
    fn validate_rejects_http2_upstream_protocol_for_cleartext_peers() {
        let mut config = base_config();
        config.upstreams[0].protocol = UpstreamProtocolConfig::Http2;

        let error =
            validate(&config).expect_err("cleartext peers should be rejected for upstream http2");
        assert!(
            error
                .to_string()
                .contains("protocol `Http2` currently requires all peers to use `https://`")
        );
    }

    fn sample_vhost(server_names: Vec<&str>) -> VirtualHostConfig {
        VirtualHostConfig {
            server_names: server_names.into_iter().map(str::to_string).collect(),
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Exact("/".to_string()),
                handler: HandlerConfig::Static {
                    status: Some(200),
                    content_type: Some("text/plain; charset=utf-8".to_string()),
                    body: "vhost\n".to_string(),
                },
                grpc_service: None,

                grpc_method: None,

                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: None,
                burst: None,
            }],
            tls: None,
        }
    }
}
