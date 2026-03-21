use std::collections::HashSet;
use std::net::IpAddr;

use http::uri::PathAndQuery;
use ipnet::IpNet;
use rginx_core::{Error, Result};

use crate::model::{Config, HandlerConfig, MatcherConfig, ServerTlsConfig, UpstreamTlsConfig, VirtualHostConfig};

pub fn validate(config: &Config) -> Result<()> {
    if config.runtime.shutdown_timeout_secs == 0 {
        return Err(Error::Config(
            "runtime.shutdown_timeout_secs must be greater than 0".to_string(),
        ));
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
        }

        if let Some(UpstreamTlsConfig::CustomCa { ca_cert_path }) = &upstream.tls {
            if ca_cert_path.trim().is_empty() {
                return Err(Error::Config(format!(
                    "upstream `{}` custom CA path must not be empty",
                    upstream.name
                )));
            }
        }

        if let Some(server_name_override) = &upstream.server_name_override {
            if server_name_override.trim().is_empty() {
                return Err(Error::Config(format!(
                    "upstream `{}` server_name_override must not be empty",
                    upstream.name
                )));
            }
        }

        if upstream.request_timeout_secs.is_some_and(|timeout| timeout == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` request_timeout_secs must be greater than 0",
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

        let has_active_health_overrides = upstream.health_check_interval_secs.is_some()
            || upstream.health_check_timeout_secs.is_some()
            || upstream.healthy_successes_required.is_some();
        if upstream.health_check_path.is_none() && has_active_health_overrides {
            return Err(Error::Config(format!(
                "upstream `{}` active health-check tuning requires health_check_path to be set",
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

        if let MatcherConfig::Exact(path) = &location.matcher {
            if !exact_routes.insert(path.clone()) {
                return Err(Error::Config(format!("duplicate exact route `{path}`")));
            }
        }

        validate_route_cidrs(matcher_label, "allow_cidrs", &location.allow_cidrs)?;
        validate_route_cidrs(matcher_label, "deny_cidrs", &location.deny_cidrs)?;
        validate_route_rate_limit(matcher_label, location.requests_per_sec, location.burst)?;

        if let HandlerConfig::Proxy { upstream, strip_prefix, proxy_set_headers, .. } = &location.handler {
            if upstream.trim().is_empty() {
                return Err(Error::Config("proxy upstream name must not be empty".to_string()));
            }

            if !upstream_names.contains(upstream) {
                return Err(Error::Config(format!("proxy upstream `{upstream}` is not defined")));
            }

            if let Some(prefix) = strip_prefix {
                if !prefix.starts_with('/') {
                    return Err(Error::Config(format!(
                        "route matcher `{matcher_label}` strip_prefix must start with `/`"
                    )));
                }
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

        if let HandlerConfig::File { root, index, try_files } = &location.handler {
            if root.trim().is_empty() {
                return Err(Error::Config(format!(
                    "route matcher `{matcher_label}` file root must not be empty"
                )));
            }

            if let Some(index) = index {
                if index.trim().is_empty() {
                    return Err(Error::Config(format!(
                        "route matcher `{matcher_label}` file index must not be empty"
                    )));
                }
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

    validate_virtual_hosts(&config.servers, &upstream_names)?;

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

fn validate_virtual_hosts(vhosts: &[VirtualHostConfig], upstream_names: &HashSet<String>) -> Result<()> {
    let mut all_server_names: HashSet<String> = HashSet::new();

    for (idx, vhost) in vhosts.iter().enumerate() {
        let vhost_label = format!("servers[{idx}]");

        for name in &vhost.server_names {
            let normalized = name.trim().to_lowercase();
            if normalized.is_empty() {
                return Err(Error::Config(format!(
                    "{vhost_label} server_name must not be empty"
                )));
            }

            if normalized.contains('/') {
                return Err(Error::Config(format!(
                    "{vhost_label} server_name `{name}` should not contain path separator"
                )));
            }

            if !all_server_names.insert(normalized) {
                return Err(Error::Config(format!(
                    "duplicate server_name `{name}` across servers"
                )));
            }
        }

        if vhost.locations.is_empty() {
            return Err(Error::Config(format!(
                "{vhost_label} must have at least one location"
            )));
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

            if let MatcherConfig::Exact(path) = &location.matcher {
                if !vhost_exact_routes.insert(path.clone()) {
                    return Err(Error::Config(format!(
                        "{vhost_label} duplicate exact route `{path}`"
                    )));
                }
            }

            validate_route_cidrs(matcher_label, "allow_cidrs", &location.allow_cidrs)?;
            validate_route_cidrs(matcher_label, "deny_cidrs", &location.deny_cidrs)?;
            validate_route_rate_limit(matcher_label, location.requests_per_sec, location.burst)?;

            if let HandlerConfig::Proxy { upstream, strip_prefix, proxy_set_headers, .. } = &location.handler {
                if upstream.trim().is_empty() {
                    return Err(Error::Config(
                        "proxy upstream name must not be empty".to_string(),
                    ));
                }

                if !upstream_names.contains(upstream) {
                    return Err(Error::Config(format!(
                        "{vhost_label} proxy upstream `{upstream}` is not defined"
                    )));
                }

                if let Some(prefix) = strip_prefix {
                    if !prefix.starts_with('/') {
                        return Err(Error::Config(format!(
                            "{vhost_label} route matcher `{matcher_label}` strip_prefix must start with `/`"
                        )));
                    }
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

            if let HandlerConfig::File { root, index, try_files } = &location.handler {
                if root.trim().is_empty() {
                    return Err(Error::Config(format!(
                        "{vhost_label} route matcher `{matcher_label}` file root must not be empty"
                    )));
                }

                if let Some(index) = index {
                    if index.trim().is_empty() {
                        return Err(Error::Config(format!(
                            "{vhost_label} route matcher `{matcher_label}` file index must not be empty"
                        )));
                    }
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
        ServerTlsConfig, UpstreamConfig, UpstreamPeerConfig,
    };

    use super::validate;

    #[test]
    fn validate_rejects_zero_max_replayable_body_size() {
        let mut config = base_config();
        config.upstreams[0].max_replayable_request_body_bytes = Some(0);

        let error = validate(&config).expect_err("zero body size should be rejected");
        assert!(error
            .to_string()
            .contains("max_replayable_request_body_bytes must be greater than 0"));
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
        assert!(error
            .to_string()
            .contains("server header_read_timeout_secs must be greater than 0"));
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

    fn base_config() -> Config {
        Config {
            runtime: RuntimeConfig { shutdown_timeout_secs: 10 },
            server: ServerConfig {
                listen: "127.0.0.1:8080".to_string(),
                server_names: Vec::new(),
                trusted_proxies: Vec::new(),
                keep_alive: None,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout_secs: None,
                tls: None,
            },
            upstreams: vec![UpstreamConfig {
                name: "backend".to_string(),
                peers: vec![UpstreamPeerConfig { url: "http://127.0.0.1:9000".to_string() }],
                tls: None,
                server_name_override: None,
                request_timeout_secs: None,
                max_replayable_request_body_bytes: None,
                unhealthy_after_failures: None,
                unhealthy_cooldown_secs: None,
                health_check_path: None,
                health_check_interval_secs: None,
                health_check_timeout_secs: None,
                healthy_successes_required: None,
            }],
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Prefix("/".to_string()),
                handler: HandlerConfig::Proxy { upstream: "backend".to_string(), preserve_host: None, strip_prefix: None, proxy_set_headers: std::collections::HashMap::new() },
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
        assert!(error
            .to_string()
            .contains("active health-check tuning requires health_check_path"));
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
    fn validate_rejects_zero_healthy_successes_required() {
        let mut config = base_config();
        config.upstreams[0].health_check_path = Some("/healthz".to_string());
        config.upstreams[0].healthy_successes_required = Some(0);

        let error = validate(&config).expect_err("zero recovery threshold should be rejected");
        assert!(error.to_string().contains("healthy_successes_required must be greater than 0"));
    }
}
