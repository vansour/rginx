use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use http::StatusCode;
use rginx_core::{
    ConfigSnapshot, Error, ProxyTarget, Result, Route, RouteAction, RouteMatcher, RuntimeSettings,
    Server, StaticResponse, Upstream, UpstreamPeer, UpstreamTls,
};
use rustls::pki_types::ServerName;

use crate::model::{Config, HandlerConfig, MatcherConfig, UpstreamTlsConfig};
use crate::validate::validate;

pub fn compile(raw: Config) -> Result<ConfigSnapshot> {
    compile_with_base(raw, Path::new("."))
}

pub fn compile_with_base(raw: Config, base_dir: impl AsRef<Path>) -> Result<ConfigSnapshot> {
    validate(&raw)?;
    let base_dir = base_dir.as_ref();

    let Config { runtime, server, upstreams: raw_upstreams, locations } = raw;

    let listen_addr = server.listen.parse()?;
    let upstreams = raw_upstreams
        .into_iter()
        .map(|upstream| {
            let crate::model::UpstreamConfig { name, peers, tls, server_name_override } = upstream;

            let peers = peers
                .into_iter()
                .map(|peer| compile_peer(&name, peer.url))
                .collect::<Result<Vec<_>>>()?;
            let tls = compile_tls(&name, tls, base_dir)?;
            let server_name_override = compile_server_name_override(&name, server_name_override)?;

            let compiled = Arc::new(Upstream::new(name.clone(), peers, tls, server_name_override));
            Ok((name, compiled))
        })
        .collect::<Result<HashMap<_, _>>>()?;

    let mut routes = locations
        .into_iter()
        .map(|location| {
            let matcher = match location.matcher {
                MatcherConfig::Exact(path) => RouteMatcher::Exact(path),
                MatcherConfig::Prefix(path) => RouteMatcher::Prefix(path),
            };

            let action = match location.handler {
                HandlerConfig::Static { status, content_type, body } => {
                    RouteAction::Static(StaticResponse {
                        status: StatusCode::from_u16(status.unwrap_or(200))?,
                        content_type: content_type
                            .unwrap_or_else(|| "text/plain; charset=utf-8".to_string()),
                        body,
                    })
                }
                HandlerConfig::Proxy { upstream } => {
                    let compiled = upstreams.get(&upstream).cloned().ok_or_else(|| {
                        Error::Config(format!("proxy upstream `{upstream}` is not defined"))
                    })?;

                    RouteAction::Proxy(ProxyTarget { upstream_name: upstream, upstream: compiled })
                }
            };

            Ok(Route { matcher, action })
        })
        .collect::<Result<Vec<_>>>()?;

    routes.sort_by(|left, right| right.matcher.priority().cmp(&left.matcher.priority()));

    Ok(ConfigSnapshot {
        runtime: RuntimeSettings {
            shutdown_timeout: Duration::from_secs(runtime.shutdown_timeout_secs),
        },
        server: Server { listen_addr },
        routes,
        upstreams,
    })
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::model::{
        Config, HandlerConfig, LocationConfig, MatcherConfig, RuntimeConfig, ServerConfig,
        UpstreamConfig, UpstreamPeerConfig, UpstreamTlsConfig,
    };

    use super::{compile, compile_with_base};

    #[test]
    fn compile_accepts_https_upstreams() {
        let config = Config {
            runtime: RuntimeConfig { shutdown_timeout_secs: 10 },
            server: ServerConfig { listen: "127.0.0.1:8080".to_string() },
            upstreams: vec![UpstreamConfig {
                name: "secure-backend".to_string(),
                peers: vec![UpstreamPeerConfig { url: "https://example.com".to_string() }],
                tls: None,
                server_name_override: None,
            }],
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Prefix("/api".to_string()),
                handler: HandlerConfig::Proxy { upstream: "secure-backend".to_string() },
            }],
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
            server: ServerConfig { listen: "127.0.0.1:8080".to_string() },
            upstreams: vec![UpstreamConfig {
                name: "dev-backend".to_string(),
                peers: vec![UpstreamPeerConfig { url: "https://localhost:9443".to_string() }],
                tls: Some(UpstreamTlsConfig::CustomCa { ca_cert_path: "dev-ca.pem".to_string() }),
                server_name_override: Some("dev.internal".to_string()),
            }],
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Prefix("/".to_string()),
                handler: HandlerConfig::Proxy { upstream: "dev-backend".to_string() },
            }],
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

        fs::remove_file(&ca_path).expect("temp CA file should be removed");
        fs::remove_dir(&base_dir).expect("temp base dir should be removed");
    }

    #[test]
    fn compile_normalizes_server_name_override() {
        let config = Config {
            runtime: RuntimeConfig { shutdown_timeout_secs: 10 },
            server: ServerConfig { listen: "127.0.0.1:8080".to_string() },
            upstreams: vec![UpstreamConfig {
                name: "secure-backend".to_string(),
                peers: vec![UpstreamPeerConfig { url: "https://[::1]:9443".to_string() }],
                tls: None,
                server_name_override: Some("[::1]".to_string()),
            }],
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Prefix("/".to_string()),
                handler: HandlerConfig::Proxy { upstream: "secure-backend".to_string() },
            }],
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
            server: ServerConfig { listen: "127.0.0.1:8080".to_string() },
            upstreams: vec![UpstreamConfig {
                name: "secure-backend".to_string(),
                peers: vec![UpstreamPeerConfig { url: "https://127.0.0.1:9443".to_string() }],
                tls: None,
                server_name_override: Some("bad name".to_string()),
            }],
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Prefix("/".to_string()),
                handler: HandlerConfig::Proxy { upstream: "secure-backend".to_string() },
            }],
        };

        let error = compile(config).expect_err("invalid override should be rejected");
        assert!(error.to_string().contains("server_name_override"));
    }
}
