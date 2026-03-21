use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use ipnet::IpNet;
use rginx_core::{
    ActiveHealthCheck, ConfigSnapshot, Error, FileTarget, ProxyTarget, ReturnAction, Result, Route, RouteAccessControl,
    RouteAction, RouteMatcher, RouteRateLimit, RuntimeSettings, Server, ServerTls, StaticResponse,
    Upstream, UpstreamPeer, UpstreamTls, VirtualHost,
};
use crate::model::{
    Config, HandlerConfig, MatcherConfig, ServerTlsConfig, UpstreamTlsConfig, VirtualHostConfig,
};
use rustls::pki_types::ServerName;

use crate::validate::validate;

const DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS: u64 = 30;
const DEFAULT_MAX_REPLAYABLE_REQUEST_BODY_BYTES: u64 = 64 * 1024;
const DEFAULT_UNHEALTHY_AFTER_FAILURES: u32 = 2;
const DEFAULT_UNHEALTHY_COOLDOWN_SECS: u64 = 10;
const DEFAULT_HEALTH_CHECK_INTERVAL_SECS: u64 = 5;
const DEFAULT_HEALTH_CHECK_TIMEOUT_SECS: u64 = 2;
const DEFAULT_HEALTHY_SUCCESSES_REQUIRED: u32 = 2;

pub fn compile(raw: Config) -> Result<ConfigSnapshot> {
    compile_with_base(raw, Path::new("."))
}

pub fn compile_with_base(raw: Config, base_dir: impl AsRef<Path>) -> Result<ConfigSnapshot> {
    validate(&raw)?;
    let base_dir = base_dir.as_ref();

    let Config { runtime, server, upstreams: raw_upstreams, locations, servers: raw_servers } = raw;
    let crate::model::ServerConfig {
        listen,
        server_names,
        trusted_proxies,
        keep_alive,
        max_headers,
        max_request_body_bytes,
        max_connections,
        header_read_timeout_secs,
        tls,
    } = server;

    let listen_addr = listen.parse()?;
    let trusted_proxies = compile_trusted_proxies(trusted_proxies)?;
    let keep_alive = keep_alive.unwrap_or(true);
    let max_headers = compile_max_headers(max_headers)?;
    let max_request_body_bytes = compile_max_request_body_bytes(max_request_body_bytes)?;
    let max_connections = compile_max_connections(max_connections)?;
    let header_read_timeout = header_read_timeout_secs.map(Duration::from_secs);
    let server_tls = compile_server_tls(tls, base_dir)?;
    let upstreams = raw_upstreams
        .into_iter()
        .map(|upstream| {
            let crate::model::UpstreamConfig {
                name,
                peers,
                tls,
                server_name_override,
                request_timeout_secs,
                max_replayable_request_body_bytes,
                unhealthy_after_failures,
                unhealthy_cooldown_secs,
                health_check_path,
                health_check_interval_secs,
                health_check_timeout_secs,
                healthy_successes_required,
            } = upstream;

            let peers = peers
                .into_iter()
                .map(|peer| compile_peer(&name, peer.url))
                .collect::<Result<Vec<_>>>()?;
            let tls = compile_tls(&name, tls, base_dir)?;
            let server_name_override = compile_server_name_override(&name, server_name_override)?;
            let request_timeout = Duration::from_secs(
                request_timeout_secs.unwrap_or(DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS),
            );
            let max_replayable_request_body_bytes = compile_max_replayable_request_body_bytes(
                &name,
                max_replayable_request_body_bytes,
            )?;
            let unhealthy_after_failures =
                unhealthy_after_failures.unwrap_or(DEFAULT_UNHEALTHY_AFTER_FAILURES);
            let unhealthy_cooldown = Duration::from_secs(
                unhealthy_cooldown_secs.unwrap_or(DEFAULT_UNHEALTHY_COOLDOWN_SECS),
            );
            let active_health_check = compile_active_health_check(
                &name,
                health_check_path,
                health_check_interval_secs,
                health_check_timeout_secs,
                healthy_successes_required,
            )?;

            let compiled = Arc::new(Upstream::new(
                name.clone(),
                peers,
                tls,
                server_name_override,
                request_timeout,
                max_replayable_request_body_bytes,
                unhealthy_after_failures,
                unhealthy_cooldown,
                active_health_check,
            ));
            Ok((name, compiled))
        })
        .collect::<Result<HashMap<_, _>>>()?;

    let mut routes = locations
        .into_iter()
        .map(|location| {
            let crate::model::LocationConfig {
                matcher,
                handler,
                allow_cidrs,
                deny_cidrs,
                requests_per_sec,
                burst,
            } = location;
            let matcher = match matcher {
                MatcherConfig::Exact(path) => RouteMatcher::Exact(path),
                MatcherConfig::Prefix(path) => RouteMatcher::Prefix(path),
            };
            let access_control = compile_route_access_control(&matcher, allow_cidrs, deny_cidrs)?;
            let rate_limit = compile_route_rate_limit(&matcher, requests_per_sec, burst)?;

            let action = match handler {
                HandlerConfig::Static { status, content_type, body } => {
                    RouteAction::Static(StaticResponse {
                        status: StatusCode::from_u16(status.unwrap_or(200))?,
                        content_type: content_type
                            .unwrap_or_else(|| "text/plain; charset=utf-8".to_string()),
                        body,
                    })
                }
                HandlerConfig::Proxy { upstream, preserve_host, strip_prefix, proxy_set_headers } => {
                    let compiled = upstreams.get(&upstream).cloned().ok_or_else(|| {
                        Error::Config(format!("proxy upstream `{upstream}` is not defined"))
                    })?;

                    let preserve_host = preserve_host.unwrap_or(false);

                    let strip_prefix = strip_prefix.and_then(|s| if s.is_empty() { None } else { Some(s) });

                    let proxy_set_headers = proxy_set_headers
                        .into_iter()
                        .map(|(name, value)| {
                            let header_name = name.parse::<http::header::HeaderName>().map_err(|e| {
                                Error::Config(format!("invalid header name `{name}`: {e}"))
                            })?;
                            let header_value = value.parse::<http::header::HeaderValue>().map_err(|e| {
                                Error::Config(format!("invalid header value for `{name}`: {e}"))
                            })?;
                            Ok((header_name, header_value))
                        })
                        .collect::<Result<Vec<_>>>()?;

                    RouteAction::Proxy(ProxyTarget {
                        upstream_name: upstream,
                        upstream: compiled,
                        preserve_host,
                        strip_prefix,
                        proxy_set_headers,
                    })
                }
                HandlerConfig::File { root, index, try_files } => {
                    let root = resolve_path(base_dir, root);
                    RouteAction::File(FileTarget {
                        root,
                        index: index.and_then(|s| if s.trim().is_empty() { None } else { Some(s) }),
                        try_files: try_files.unwrap_or_default(),
                    })
                }
                HandlerConfig::Return { status, location, body } => {
                    RouteAction::Return(ReturnAction {
                        status: StatusCode::from_u16(status)?,
                        location,
                        body,
                    })
                }
                HandlerConfig::Status => RouteAction::Status,
                HandlerConfig::Metrics => RouteAction::Metrics,
            };

            Ok(Route { matcher, action, access_control, rate_limit })
        })
        .collect::<Result<Vec<_>>>()?;

    routes.sort_by(|left, right| right.matcher.priority().cmp(&left.matcher.priority()));

    let default_vhost = VirtualHost { server_names, routes: routes.clone(), tls: server_tls.clone() };

    let vhosts = raw_servers
        .into_iter()
        .map(|vhost_config| compile_virtual_host(vhost_config, &upstreams, base_dir))
        .collect::<Result<Vec<_>>>()?;

    Ok(ConfigSnapshot {
        runtime: RuntimeSettings {
            shutdown_timeout: Duration::from_secs(runtime.shutdown_timeout_secs),
        },
        server: Server {
            listen_addr,
            trusted_proxies,
            keep_alive,
            max_headers,
            max_request_body_bytes,
            max_connections,
            header_read_timeout,
            tls: server_tls,
        },
        default_vhost,
        vhosts,
        routes,
        upstreams,
    })
}

fn compile_max_headers(max_headers: Option<u64>) -> Result<Option<usize>> {
    max_headers
        .map(|limit| {
            usize::try_from(limit).map_err(|_| {
                Error::Config(format!("server max_headers `{limit}` exceeds platform limits"))
            })
        })
        .transpose()
}

fn compile_max_request_body_bytes(max_request_body_bytes: Option<u64>) -> Result<Option<usize>> {
    max_request_body_bytes
        .map(|limit| {
            usize::try_from(limit).map_err(|_| {
                Error::Config(format!(
                    "server max_request_body_bytes `{limit}` exceeds platform limits"
                ))
            })
        })
        .transpose()
}

fn compile_max_connections(max_connections: Option<u64>) -> Result<Option<usize>> {
    max_connections
        .map(|limit| {
            usize::try_from(limit).map_err(|_| {
                Error::Config(format!("server max_connections `{limit}` exceeds platform limits"))
            })
        })
        .transpose()
}

fn compile_peer(upstream_name: &str, url: String) -> Result<UpstreamPeer> {
    let uri: http::Uri = url.parse()?;
    let scheme = uri.scheme_str().ok_or_else(|| {
        Error::Config(format!("upstream `{upstream_name}` peer `{url}` must include a scheme"))
    })?;
    let authority = uri.authority().ok_or_else(|| {
        Error::Config(format!("upstream `{upstream_name}` peer `{url}` must include an authority"))
    })?;

    if scheme != "http" && scheme != "https" {
        return Err(Error::Config(format!(
            "upstream `{upstream_name}` peer `{url}` uses unsupported scheme `{scheme}`; only `http` and `https` are supported in this build"
        )));
    }

    if uri.path() != "/" && !uri.path().is_empty() {
        return Err(Error::Config(format!(
            "upstream `{upstream_name}` peer `{url}` must not contain a path"
        )));
    }

    if uri.query().is_some() {
        return Err(Error::Config(format!(
            "upstream `{upstream_name}` peer `{url}` must not contain a query"
        )));
    }

    Ok(UpstreamPeer { url, scheme: scheme.to_string(), authority: authority.to_string() })
}

fn compile_tls(
    upstream_name: &str,
    tls: Option<UpstreamTlsConfig>,
    base_dir: &Path,
) -> Result<UpstreamTls> {
    match tls.unwrap_or(UpstreamTlsConfig::NativeRoots) {
        UpstreamTlsConfig::NativeRoots => Ok(UpstreamTls::NativeRoots),
        UpstreamTlsConfig::Insecure => Ok(UpstreamTls::Insecure),
        UpstreamTlsConfig::CustomCa { ca_cert_path } => {
            let resolved = resolve_path(base_dir, ca_cert_path);
            if !resolved.is_file() {
                return Err(Error::Config(format!(
                    "upstream `{upstream_name}` custom CA file `{}` does not exist or is not a file",
                    resolved.display()
                )));
            }

            Ok(UpstreamTls::CustomCa { ca_cert_path: resolved })
        }
    }
}

fn compile_server_tls(tls: Option<ServerTlsConfig>, base_dir: &Path) -> Result<Option<ServerTls>> {
    let Some(ServerTlsConfig { cert_path, key_path }) = tls else {
        return Ok(None);
    };

    let cert_path = resolve_path(base_dir, cert_path);
    if !cert_path.is_file() {
        return Err(Error::Config(format!(
            "server TLS certificate file `{}` does not exist or is not a file",
            cert_path.display()
        )));
    }

    let key_path = resolve_path(base_dir, key_path);
    if !key_path.is_file() {
        return Err(Error::Config(format!(
            "server TLS private key file `{}` does not exist or is not a file",
            key_path.display()
        )));
    }

    Ok(Some(ServerTls { cert_path, key_path }))
}

fn compile_virtual_host(
    config: VirtualHostConfig,
    upstreams: &HashMap<String, Arc<Upstream>>,
    base_dir: &Path,
) -> Result<VirtualHost> {
    let VirtualHostConfig { server_names, locations, tls } = config;

    let mut routes = locations
        .into_iter()
        .map(|location| {
            let crate::model::LocationConfig {
                matcher,
                handler,
                allow_cidrs,
                deny_cidrs,
                requests_per_sec,
                burst,
            } = location;
            let matcher = match matcher {
                MatcherConfig::Exact(path) => RouteMatcher::Exact(path),
                MatcherConfig::Prefix(path) => RouteMatcher::Prefix(path),
            };
            let access_control = compile_route_access_control(&matcher, allow_cidrs, deny_cidrs)?;
            let rate_limit = compile_route_rate_limit(&matcher, requests_per_sec, burst)?;

            let action = match handler {
                HandlerConfig::Static { status, content_type, body } => {
                    RouteAction::Static(StaticResponse {
                        status: StatusCode::from_u16(status.unwrap_or(200))?,
                        content_type: content_type
                            .unwrap_or_else(|| "text/plain; charset=utf-8".to_string()),
                        body,
                    })
                }
                HandlerConfig::Proxy { upstream, preserve_host, strip_prefix, proxy_set_headers } => {
                    let compiled = upstreams.get(&upstream).cloned().ok_or_else(|| {
                        Error::Config(format!("proxy upstream `{upstream}` is not defined"))
                    })?;

                    let preserve_host = preserve_host.unwrap_or(false);

                    let strip_prefix = strip_prefix.and_then(|s| if s.is_empty() { None } else { Some(s) });

                    let proxy_set_headers = proxy_set_headers
                        .into_iter()
                        .map(|(name, value)| {
                            let header_name = name.parse::<http::header::HeaderName>().map_err(|e| {
                                Error::Config(format!("invalid header name `{name}`: {e}"))
                            })?;
                            let header_value = value.parse::<http::header::HeaderValue>().map_err(|e| {
                                Error::Config(format!("invalid header value for `{name}`: {e}"))
                            })?;
                            Ok((header_name, header_value))
                        })
                        .collect::<Result<Vec<_>>>()?;

                    RouteAction::Proxy(ProxyTarget {
                        upstream_name: upstream,
                        upstream: compiled,
                        preserve_host,
                        strip_prefix,
                        proxy_set_headers,
                    })
                }
                HandlerConfig::File { root, index, try_files } => {
                    let root = resolve_path(base_dir, root);
                    RouteAction::File(FileTarget {
                        root,
                        index: index.and_then(|s| if s.trim().is_empty() { None } else { Some(s) }),
                        try_files: try_files.unwrap_or_default(),
                    })
                }
                HandlerConfig::Return { status, location, body } => {
                    RouteAction::Return(ReturnAction {
                        status: StatusCode::from_u16(status)?,
                        location,
                        body,
                    })
                }
                HandlerConfig::Status => RouteAction::Status,
                HandlerConfig::Metrics => RouteAction::Metrics,
            };

            Ok(Route { matcher, action, access_control, rate_limit })
        })
        .collect::<Result<Vec<_>>>()?;

    routes.sort_by(|left, right| right.matcher.priority().cmp(&left.matcher.priority()));

    let tls = compile_server_tls(tls, base_dir)?;

    Ok(VirtualHost { server_names, routes, tls })
}

fn compile_server_name_override(
    upstream_name: &str,
    server_name_override: Option<String>,
) -> Result<Option<String>> {
    let Some(server_name_override) = server_name_override else {
        return Ok(None);
    };

    let normalized = normalize_server_name_override(&server_name_override);
    ServerName::try_from(normalized.clone()).map_err(|error| {
        Error::Config(format!(
            "upstream `{upstream_name}` server_name_override `{normalized}` is invalid: {error}"
        ))
    })?;

    Ok(Some(normalized))
}

fn compile_max_replayable_request_body_bytes(
    upstream_name: &str,
    max_replayable_request_body_bytes: Option<u64>,
) -> Result<usize> {
    let bytes =
        max_replayable_request_body_bytes.unwrap_or(DEFAULT_MAX_REPLAYABLE_REQUEST_BODY_BYTES);
    usize::try_from(bytes).map_err(|_| {
        Error::Config(format!(
            "upstream `{upstream_name}` max_replayable_request_body_bytes `{bytes}` exceeds platform limits"
        ))
    })
}

fn compile_active_health_check(
    upstream_name: &str,
    health_check_path: Option<String>,
    health_check_interval_secs: Option<u64>,
    health_check_timeout_secs: Option<u64>,
    healthy_successes_required: Option<u32>,
) -> Result<Option<ActiveHealthCheck>> {
    let Some(path) = health_check_path else {
        return Ok(None);
    };

    http::uri::PathAndQuery::from_str(&path).map_err(|error| {
        Error::Config(format!(
            "upstream `{upstream_name}` health_check_path `{path}` is invalid: {error}"
        ))
    })?;

    Ok(Some(ActiveHealthCheck {
        path,
        interval: Duration::from_secs(
            health_check_interval_secs.unwrap_or(DEFAULT_HEALTH_CHECK_INTERVAL_SECS),
        ),
        timeout: Duration::from_secs(
            health_check_timeout_secs.unwrap_or(DEFAULT_HEALTH_CHECK_TIMEOUT_SECS),
        ),
        healthy_successes_required: healthy_successes_required
            .unwrap_or(DEFAULT_HEALTHY_SUCCESSES_REQUIRED),
    }))
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

fn compile_trusted_proxies(values: Vec<String>) -> Result<Vec<IpNet>> {
    values
        .into_iter()
        .map(|value| {
            let normalized = normalize_trusted_proxy(&value).ok_or_else(|| {
                Error::Config(format!(
                    "server trusted_proxies entry `{value}` must be a valid IP address or CIDR"
                ))
            })?;

            normalized.parse::<IpNet>().map_err(|error| {
                Error::Config(format!("server trusted_proxies entry `{value}` is invalid: {error}"))
            })
        })
        .collect()
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

fn normalize_server_name_override(value: &str) -> String {
    let trimmed = value.trim();
    trimmed
        .strip_prefix('[')
        .and_then(|candidate| candidate.strip_suffix(']'))
        .unwrap_or(trimmed)
        .to_string()
}

fn resolve_path(base_dir: &Path, path: String) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        base_dir.join(path)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::model::{
        Config, HandlerConfig, LocationConfig, MatcherConfig, RuntimeConfig, ServerConfig,
        ServerTlsConfig, UpstreamConfig, UpstreamPeerConfig, UpstreamTlsConfig,
    };

    use super::{
        compile, compile_with_base, DEFAULT_HEALTHY_SUCCESSES_REQUIRED,
        DEFAULT_HEALTH_CHECK_INTERVAL_SECS, DEFAULT_HEALTH_CHECK_TIMEOUT_SECS,
        DEFAULT_MAX_REPLAYABLE_REQUEST_BODY_BYTES, DEFAULT_UNHEALTHY_AFTER_FAILURES,
        DEFAULT_UNHEALTHY_COOLDOWN_SECS, DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS,
    };

    #[test]
    fn compile_accepts_https_upstreams() {
        let config = Config {
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
                name: "secure-backend".to_string(),
                peers: vec![UpstreamPeerConfig { url: "https://example.com".to_string() }],
                tls: None,
                server_name_override: None,
                request_timeout_secs: None,
                max_replayable_request_body_bytes: None,
                unhealthy_after_failures: None,
                unhealthy_cooldown_secs: None,
                health_check_path: Some("/healthz".to_string()),
                health_check_interval_secs: None,
                health_check_timeout_secs: None,
                healthy_successes_required: None,
            }],
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Prefix("/api".to_string()),
                handler: HandlerConfig::Proxy { upstream: "secure-backend".to_string(), preserve_host: None, strip_prefix: None, proxy_set_headers: std::collections::HashMap::new() },
                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: None,
                burst: None,
            }],
            servers: Vec::new(),
        };

        let snapshot = compile(config).expect("https upstream should compile");
        let proxy = match &snapshot.routes[0].action {
            rginx_core::RouteAction::Proxy(proxy) => proxy,
            _ => panic!("expected proxy route"),
        };

        let peer = proxy.upstream.next_peer().expect("expected one upstream peer");
        assert_eq!(proxy.upstream_name, "secure-backend");
        assert_eq!(peer.scheme, "https");
        assert_eq!(peer.authority, "example.com");
        assert_eq!(
            proxy.upstream.request_timeout,
            Duration::from_secs(DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS)
        );
        assert_eq!(
            proxy.upstream.max_replayable_request_body_bytes,
            DEFAULT_MAX_REPLAYABLE_REQUEST_BODY_BYTES as usize
        );
        assert_eq!(proxy.upstream.unhealthy_after_failures, DEFAULT_UNHEALTHY_AFTER_FAILURES);
        assert_eq!(
            proxy.upstream.unhealthy_cooldown,
            Duration::from_secs(DEFAULT_UNHEALTHY_COOLDOWN_SECS)
        );
        let active_health = proxy
            .upstream
            .active_health_check
            .as_ref()
            .expect("active health-check config should compile");
        assert_eq!(active_health.path, "/healthz");
        assert_eq!(active_health.interval, Duration::from_secs(DEFAULT_HEALTH_CHECK_INTERVAL_SECS));
        assert_eq!(active_health.timeout, Duration::from_secs(DEFAULT_HEALTH_CHECK_TIMEOUT_SECS));
        assert_eq!(active_health.healthy_successes_required, DEFAULT_HEALTHY_SUCCESSES_REQUIRED);
    }

    #[test]
    fn compile_resolves_custom_ca_relative_to_config_base() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let base_dir = std::env::temp_dir().join(format!("rginx-config-test-{unique}"));
        fs::create_dir_all(&base_dir).expect("temp base dir should be created");
        let ca_path = base_dir.join("dev-ca.pem");
        fs::write(&ca_path, b"placeholder").expect("temp CA file should be written");

        let config = Config {
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
                name: "dev-backend".to_string(),
                peers: vec![UpstreamPeerConfig { url: "https://localhost:9443".to_string() }],
                tls: Some(UpstreamTlsConfig::CustomCa { ca_cert_path: "dev-ca.pem".to_string() }),
                server_name_override: Some("dev.internal".to_string()),
                request_timeout_secs: Some(5),
                max_replayable_request_body_bytes: Some(1024),
                unhealthy_after_failures: Some(3),
                unhealthy_cooldown_secs: Some(15),
                health_check_path: Some("/ready".to_string()),
                health_check_interval_secs: Some(7),
                health_check_timeout_secs: Some(3),
                healthy_successes_required: Some(4),
            }],
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Prefix("/".to_string()),
                handler: HandlerConfig::Proxy { upstream: "dev-backend".to_string(), preserve_host: None, strip_prefix: None, proxy_set_headers: std::collections::HashMap::new() },
                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: None,
                burst: None,
            }],
            servers: Vec::new(),
        };

        let snapshot =
            compile_with_base(config, &base_dir).expect("custom CA config should compile");
        let proxy = match &snapshot.routes[0].action {
            rginx_core::RouteAction::Proxy(proxy) => proxy,
            _ => panic!("expected proxy route"),
        };

        assert!(matches!(
            &proxy.upstream.tls,
            rginx_core::UpstreamTls::CustomCa { ca_cert_path } if ca_cert_path == &ca_path
        ));
        assert_eq!(proxy.upstream.server_name_override.as_deref(), Some("dev.internal"));
        assert_eq!(proxy.upstream.request_timeout, Duration::from_secs(5));
        assert_eq!(proxy.upstream.max_replayable_request_body_bytes, 1024);
        assert_eq!(proxy.upstream.unhealthy_after_failures, 3);
        assert_eq!(proxy.upstream.unhealthy_cooldown, Duration::from_secs(15));
        let active_health = proxy
            .upstream
            .active_health_check
            .as_ref()
            .expect("custom active health-check config should compile");
        assert_eq!(active_health.path, "/ready");
        assert_eq!(active_health.interval, Duration::from_secs(7));
        assert_eq!(active_health.timeout, Duration::from_secs(3));
        assert_eq!(active_health.healthy_successes_required, 4);

        fs::remove_file(&ca_path).expect("temp CA file should be removed");
        fs::remove_dir(&base_dir).expect("temp base dir should be removed");
    }

    #[test]
    fn compile_normalizes_server_name_override() {
        let config = Config {
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
                name: "secure-backend".to_string(),
                peers: vec![UpstreamPeerConfig { url: "https://[::1]:9443".to_string() }],
                tls: None,
                server_name_override: Some("[::1]".to_string()),
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
                handler: HandlerConfig::Proxy { upstream: "secure-backend".to_string(), preserve_host: None, strip_prefix: None, proxy_set_headers: std::collections::HashMap::new() },
                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: None,
                burst: None,
            }],
            servers: Vec::new(),
        };

        let snapshot = compile(config).expect("server name override should compile");
        let proxy = match &snapshot.routes[0].action {
            rginx_core::RouteAction::Proxy(proxy) => proxy,
            _ => panic!("expected proxy route"),
        };

        assert_eq!(proxy.upstream.server_name_override.as_deref(), Some("::1"));
    }

    #[test]
    fn compile_rejects_invalid_server_name_override() {
        let config = Config {
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
                name: "secure-backend".to_string(),
                peers: vec![UpstreamPeerConfig { url: "https://127.0.0.1:9443".to_string() }],
                tls: None,
                server_name_override: Some("bad name".to_string()),
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
                handler: HandlerConfig::Proxy { upstream: "secure-backend".to_string(), preserve_host: None, strip_prefix: None, proxy_set_headers: std::collections::HashMap::new() },
                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: None,
                burst: None,
            }],
            servers: Vec::new(),
        };

        let error = compile(config).expect_err("invalid override should be rejected");
        assert!(error.to_string().contains("server_name_override"));
    }

    #[test]
    fn compile_accepts_status_routes() {
        let config = Config {
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
            upstreams: Vec::new(),
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Exact("/status".to_string()),
                handler: HandlerConfig::Status,
                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: None,
                burst: None,
            }],
            servers: Vec::new(),
        };

        let snapshot = compile(config).expect("status route should compile");
        assert!(matches!(snapshot.routes[0].action, rginx_core::RouteAction::Status));
    }

    #[test]
    fn compile_accepts_metrics_routes() {
        let config = Config {
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
            upstreams: Vec::new(),
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Exact("/metrics".to_string()),
                handler: HandlerConfig::Metrics,
                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: None,
                burst: None,
            }],
            servers: Vec::new(),
        };

        let snapshot = compile(config).expect("metrics route should compile");
        assert!(matches!(snapshot.routes[0].action, rginx_core::RouteAction::Metrics));
    }

    #[test]
    fn compile_attaches_route_access_control() {
        let config = Config {
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
            upstreams: Vec::new(),
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Exact("/status".to_string()),
                handler: HandlerConfig::Status,
                allow_cidrs: vec!["127.0.0.1/32".to_string(), "::1/128".to_string()],
                deny_cidrs: vec!["127.0.0.2/32".to_string()],
                requests_per_sec: None,
                burst: None,
            }],
            servers: Vec::new(),
        };

        let snapshot = compile(config).expect("access-controlled route should compile");
        assert_eq!(snapshot.routes[0].access_control.allow_cidrs.len(), 2);
        assert_eq!(snapshot.routes[0].access_control.deny_cidrs.len(), 1);
    }

    #[test]
    fn compile_attaches_route_rate_limit() {
        let config = Config {
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
            upstreams: Vec::new(),
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Prefix("/api".to_string()),
                handler: HandlerConfig::Static {
                    status: Some(200),
                    content_type: Some("text/plain; charset=utf-8".to_string()),
                    body: "ok\n".to_string(),
                },
                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: Some(20),
                burst: Some(5),
            }],
            servers: Vec::new(),
        };

        let snapshot = compile(config).expect("rate-limited route should compile");
        let rate_limit = snapshot.routes[0].rate_limit.expect("route rate limit should exist");
        assert_eq!(rate_limit.requests_per_sec, 20);
        assert_eq!(rate_limit.burst, 5);
    }

    #[test]
    fn compile_resolves_server_tls_paths_relative_to_config_base() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let base_dir = std::env::temp_dir().join(format!("rginx-server-tls-config-test-{unique}"));
        fs::create_dir_all(&base_dir).expect("temp base dir should be created");
        let cert_path = base_dir.join("server.crt");
        let key_path = base_dir.join("server.key");
        fs::write(&cert_path, b"placeholder").expect("temp cert file should be written");
        fs::write(&key_path, b"placeholder").expect("temp key file should be written");

        let config = Config {
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
                tls: Some(ServerTlsConfig {
                    cert_path: "server.crt".to_string(),
                    key_path: "server.key".to_string(),
                }),
            },
            upstreams: Vec::new(),
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Exact("/".to_string()),
                handler: HandlerConfig::Static {
                    status: Some(200),
                    content_type: Some("text/plain; charset=utf-8".to_string()),
                    body: "ok\n".to_string(),
                },
                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: None,
                burst: None,
            }],
            servers: Vec::new(),
        };

        let snapshot = compile_with_base(config, &base_dir).expect("server TLS should compile");
        let tls = snapshot.server.tls.expect("compiled server TLS should exist");
        assert_eq!(tls.cert_path, cert_path);
        assert_eq!(tls.key_path, key_path);

        fs::remove_file(cert_path).expect("temp cert file should be removed");
        fs::remove_file(key_path).expect("temp key file should be removed");
        fs::remove_dir(base_dir).expect("temp base dir should be removed");
    }

    #[test]
    fn compile_normalizes_trusted_proxy_ips_and_cidrs() {
        let config = Config {
            runtime: RuntimeConfig { shutdown_timeout_secs: 10 },
            server: ServerConfig {
                listen: "127.0.0.1:8080".to_string(),
                server_names: Vec::new(),
                trusted_proxies: vec!["10.0.0.0/8".to_string(), "127.0.0.1".to_string()],
                keep_alive: None,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout_secs: None,
                tls: None,
            },
            upstreams: Vec::new(),
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Exact("/".to_string()),
                handler: HandlerConfig::Static {
                    status: Some(200),
                    content_type: Some("text/plain; charset=utf-8".to_string()),
                    body: "ok\n".to_string(),
                },
                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: None,
                burst: None,
            }],
            servers: Vec::new(),
        };

        let snapshot = compile(config).expect("trusted proxies should compile");
        assert_eq!(snapshot.server.trusted_proxies.len(), 2);
        assert!(snapshot.server.is_trusted_proxy("10.1.2.3".parse().unwrap()));
        assert!(snapshot.server.is_trusted_proxy("127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn compile_attaches_server_hardening_settings() {
        let config = Config {
            runtime: RuntimeConfig { shutdown_timeout_secs: 10 },
            server: ServerConfig {
                listen: "127.0.0.1:8080".to_string(),
                server_names: Vec::new(),
                trusted_proxies: Vec::new(),
                keep_alive: Some(false),
                max_headers: Some(32),
                max_request_body_bytes: Some(1024),
                max_connections: Some(256),
                header_read_timeout_secs: Some(3),
                tls: None,
            },
            upstreams: Vec::new(),
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Exact("/".to_string()),
                handler: HandlerConfig::Static {
                    status: Some(200),
                    content_type: Some("text/plain; charset=utf-8".to_string()),
                    body: "ok\n".to_string(),
                },
                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: None,
                burst: None,
            }],
            servers: Vec::new(),
        };

        let snapshot = compile(config).expect("server hardening settings should compile");
        assert!(!snapshot.server.keep_alive);
        assert_eq!(snapshot.server.max_headers, Some(32));
        assert_eq!(snapshot.server.max_request_body_bytes, Some(1024));
        assert_eq!(snapshot.server.max_connections, Some(256));
        assert_eq!(snapshot.server.header_read_timeout, Some(Duration::from_secs(3)));
    }
}
