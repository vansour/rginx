use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use rginx_core::{
    ActiveHealthCheck, ClientIdentity, Error, Result, TlsVersion, Upstream, UpstreamLoadBalance,
    UpstreamPeer, UpstreamProtocol, UpstreamSettings, UpstreamTls,
};
use rustls::pki_types::ServerName;

use crate::model::{
    TlsVersionConfig, UpstreamConfig, UpstreamLoadBalanceConfig, UpstreamProtocolConfig,
    UpstreamTlsConfig, UpstreamTlsModeConfig,
};

pub(super) fn compile_upstreams(
    raw_upstreams: Vec<UpstreamConfig>,
    base_dir: &Path,
) -> Result<HashMap<String, Arc<Upstream>>> {
    raw_upstreams
        .into_iter()
        .map(|upstream| {
            let UpstreamConfig {
                name,
                peers,
                tls,
                protocol,
                load_balance,
                server_name,
                server_name_override,
                request_timeout_secs,
                connect_timeout_secs,
                read_timeout_secs,
                write_timeout_secs,
                idle_timeout_secs,
                pool_idle_timeout_secs,
                pool_max_idle_per_host,
                tcp_keepalive_secs,
                tcp_nodelay,
                http2_keep_alive_interval_secs,
                http2_keep_alive_timeout_secs,
                http2_keep_alive_while_idle,
                max_replayable_request_body_bytes,
                unhealthy_after_failures,
                unhealthy_cooldown_secs,
                health_check_path,
                health_check_grpc_service,
                health_check_interval_secs,
                health_check_timeout_secs,
                healthy_successes_required,
            } = upstream;

            let peers = peers
                .into_iter()
                .map(|peer| compile_peer(&name, peer.url, peer.weight, peer.backup))
                .collect::<Result<Vec<_>>>()?;
            let (tls, tls_versions, client_identity) = compile_tls(&name, tls, base_dir)?;
            let protocol = compile_protocol(&name, protocol, &peers)?;
            let load_balance = compile_load_balance(load_balance);
            let server_name_override = compile_server_name_override(&name, server_name_override)?;
            let request_timeout = compile_timeout_secs(
                read_timeout_secs
                    .or(request_timeout_secs)
                    .unwrap_or(super::DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS),
                &name,
                "read_timeout_secs",
            )?;
            let connect_timeout = compile_timeout_secs(
                connect_timeout_secs
                    .or(request_timeout_secs)
                    .unwrap_or(super::DEFAULT_UPSTREAM_CONNECT_TIMEOUT_SECS),
                &name,
                "connect_timeout_secs",
            )?;
            let write_timeout = compile_timeout_secs(
                write_timeout_secs
                    .or(request_timeout_secs)
                    .unwrap_or(super::DEFAULT_UPSTREAM_WRITE_TIMEOUT_SECS),
                &name,
                "write_timeout_secs",
            )?;
            let idle_timeout = compile_timeout_secs(
                idle_timeout_secs
                    .or(request_timeout_secs)
                    .unwrap_or(super::DEFAULT_UPSTREAM_IDLE_TIMEOUT_SECS),
                &name,
                "idle_timeout_secs",
            )?;
            let pool_idle_timeout = match pool_idle_timeout_secs {
                Some(0) => None,
                Some(timeout) => {
                    Some(compile_timeout_secs(timeout, &name, "pool_idle_timeout_secs")?)
                }
                None => Some(Duration::from_secs(super::DEFAULT_UPSTREAM_POOL_IDLE_TIMEOUT_SECS)),
            };
            let pool_max_idle_per_host = compile_pool_max_idle_per_host(
                &name,
                pool_max_idle_per_host
                    .unwrap_or(super::DEFAULT_UPSTREAM_POOL_MAX_IDLE_PER_HOST as u64),
            )?;
            let tcp_keepalive = tcp_keepalive_secs
                .map(|timeout| compile_timeout_secs(timeout, &name, "tcp_keepalive_secs"))
                .transpose()?;
            let tcp_nodelay = tcp_nodelay.unwrap_or(false);
            let http2_keep_alive_interval = http2_keep_alive_interval_secs
                .map(|timeout| {
                    compile_timeout_secs(timeout, &name, "http2_keep_alive_interval_secs")
                })
                .transpose()?;
            let http2_keep_alive_timeout = compile_timeout_secs(
                http2_keep_alive_timeout_secs
                    .unwrap_or(super::DEFAULT_UPSTREAM_HTTP2_KEEP_ALIVE_TIMEOUT_SECS),
                &name,
                "http2_keep_alive_timeout_secs",
            )?;
            let http2_keep_alive_while_idle = http2_keep_alive_while_idle.unwrap_or(false);
            let max_replayable_request_body_bytes = compile_max_replayable_request_body_bytes(
                &name,
                max_replayable_request_body_bytes,
            )?;
            let unhealthy_after_failures =
                unhealthy_after_failures.unwrap_or(super::DEFAULT_UNHEALTHY_AFTER_FAILURES);
            let unhealthy_cooldown = Duration::from_secs(
                unhealthy_cooldown_secs.unwrap_or(super::DEFAULT_UNHEALTHY_COOLDOWN_SECS),
            );
            let active_health_check = compile_active_health_check(
                &name,
                health_check_path,
                health_check_grpc_service,
                health_check_interval_secs,
                health_check_timeout_secs,
                healthy_successes_required,
            )?;

            let compiled = Arc::new(Upstream::new(
                name.clone(),
                peers,
                tls,
                UpstreamSettings {
                    protocol,
                    load_balance,
                    server_name: server_name.unwrap_or(true),
                    server_name_override,
                    tls_versions,
                    client_identity,
                    request_timeout,
                    connect_timeout,
                    write_timeout,
                    idle_timeout,
                    pool_idle_timeout,
                    pool_max_idle_per_host,
                    tcp_keepalive,
                    tcp_nodelay,
                    http2_keep_alive_interval,
                    http2_keep_alive_timeout,
                    http2_keep_alive_while_idle,
                    max_replayable_request_body_bytes,
                    unhealthy_after_failures,
                    unhealthy_cooldown,
                    active_health_check,
                },
            ));
            Ok((name, compiled))
        })
        .collect()
}

fn compile_timeout_secs(raw: u64, upstream_name: &str, field: &str) -> Result<Duration> {
    if raw == 0 {
        return Err(Error::Config(format!(
            "upstream `{upstream_name}` {field} must be greater than 0"
        )));
    }

    Ok(Duration::from_secs(raw))
}

fn compile_pool_max_idle_per_host(upstream_name: &str, raw: u64) -> Result<usize> {
    usize::try_from(raw).map_err(|_| {
        Error::Config(format!(
            "upstream `{upstream_name}` pool_max_idle_per_host `{raw}` exceeds platform limits"
        ))
    })
}

fn compile_peer(
    upstream_name: &str,
    url: String,
    weight: u32,
    backup: bool,
) -> Result<UpstreamPeer> {
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

    Ok(UpstreamPeer {
        url,
        scheme: scheme.to_string(),
        authority: authority.to_string(),
        weight,
        backup,
    })
}

fn compile_tls(
    upstream_name: &str,
    tls: Option<UpstreamTlsConfig>,
    base_dir: &Path,
) -> Result<(UpstreamTls, Option<Vec<TlsVersion>>, Option<ClientIdentity>)> {
    let tls = tls.unwrap_or(UpstreamTlsConfig {
        verify: UpstreamTlsModeConfig::NativeRoots,
        versions: None,
        client_cert_path: None,
        client_key_path: None,
    });

    let verify_mode = match tls.verify {
        UpstreamTlsModeConfig::NativeRoots => UpstreamTls::NativeRoots,
        UpstreamTlsModeConfig::Insecure => UpstreamTls::Insecure,
        UpstreamTlsModeConfig::CustomCa { ca_cert_path } => {
            let resolved = super::resolve_path(base_dir, ca_cert_path);
            if !resolved.is_file() {
                return Err(Error::Config(format!(
                    "upstream `{upstream_name}` custom CA file `{}` does not exist or is not a file",
                    resolved.display()
                )));
            }

            UpstreamTls::CustomCa { ca_cert_path: resolved }
        }
    };

    let tls_versions = compile_tls_versions(&tls.versions);
    let client_identity = match (tls.client_cert_path, tls.client_key_path) {
        (None, None) => None,
        (Some(cert_path), Some(key_path)) => {
            let cert_path = super::resolve_path(base_dir, cert_path);
            if !cert_path.is_file() {
                return Err(Error::Config(format!(
                    "upstream `{upstream_name}` client certificate file `{}` does not exist or is not a file",
                    cert_path.display()
                )));
            }

            let key_path = super::resolve_path(base_dir, key_path);
            if !key_path.is_file() {
                return Err(Error::Config(format!(
                    "upstream `{upstream_name}` client private key file `{}` does not exist or is not a file",
                    key_path.display()
                )));
            }

            Some(ClientIdentity { cert_path, key_path })
        }
        _ => {
            return Err(Error::Config(format!(
                "upstream `{upstream_name}` mTLS identity requires both client_cert_path and client_key_path"
            )));
        }
    };

    Ok((verify_mode, tls_versions, client_identity))
}

fn compile_tls_versions(versions: &Option<Vec<TlsVersionConfig>>) -> Option<Vec<TlsVersion>> {
    versions.as_ref().map(|versions| {
        versions
            .iter()
            .map(|version| match version {
                TlsVersionConfig::Tls12 => TlsVersion::Tls12,
                TlsVersionConfig::Tls13 => TlsVersion::Tls13,
            })
            .collect()
    })
}

fn compile_protocol(
    upstream_name: &str,
    protocol: UpstreamProtocolConfig,
    peers: &[UpstreamPeer],
) -> Result<UpstreamProtocol> {
    match protocol {
        UpstreamProtocolConfig::Auto => Ok(UpstreamProtocol::Auto),
        UpstreamProtocolConfig::Http1 => Ok(UpstreamProtocol::Http1),
        UpstreamProtocolConfig::Http2 => {
            if peers.iter().any(|peer| peer.scheme != "https") {
                return Err(Error::Config(format!(
                    "upstream `{upstream_name}` protocol `Http2` currently requires all peers to use `https://`; cleartext h2c upstreams are not supported"
                )));
            }

            Ok(UpstreamProtocol::Http2)
        }
    }
}

fn compile_load_balance(load_balance: UpstreamLoadBalanceConfig) -> UpstreamLoadBalance {
    match load_balance {
        UpstreamLoadBalanceConfig::RoundRobin => UpstreamLoadBalance::RoundRobin,
        UpstreamLoadBalanceConfig::IpHash => UpstreamLoadBalance::IpHash,
        UpstreamLoadBalanceConfig::LeastConn => UpstreamLoadBalance::LeastConn,
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

fn compile_max_replayable_request_body_bytes(
    upstream_name: &str,
    max_replayable_request_body_bytes: Option<u64>,
) -> Result<usize> {
    let bytes = max_replayable_request_body_bytes
        .unwrap_or(super::DEFAULT_MAX_REPLAYABLE_REQUEST_BODY_BYTES);
    usize::try_from(bytes).map_err(|_| {
        Error::Config(format!(
            "upstream `{upstream_name}` max_replayable_request_body_bytes `{bytes}` exceeds platform limits"
        ))
    })
}

fn compile_active_health_check(
    upstream_name: &str,
    health_check_path: Option<String>,
    health_check_grpc_service: Option<String>,
    health_check_interval_secs: Option<u64>,
    health_check_timeout_secs: Option<u64>,
    healthy_successes_required: Option<u32>,
) -> Result<Option<ActiveHealthCheck>> {
    let path = match (health_check_path, health_check_grpc_service.as_ref()) {
        (Some(path), _) => path,
        (None, Some(_)) => super::DEFAULT_GRPC_HEALTH_CHECK_PATH.to_string(),
        (None, None) => return Ok(None),
    };

    http::uri::PathAndQuery::from_str(&path).map_err(|error| {
        Error::Config(format!(
            "upstream `{upstream_name}` health_check_path `{path}` is invalid: {error}"
        ))
    })?;

    Ok(Some(ActiveHealthCheck {
        path,
        grpc_service: health_check_grpc_service,
        interval: Duration::from_secs(
            health_check_interval_secs.unwrap_or(super::DEFAULT_HEALTH_CHECK_INTERVAL_SECS),
        ),
        timeout: Duration::from_secs(
            health_check_timeout_secs.unwrap_or(super::DEFAULT_HEALTH_CHECK_TIMEOUT_SECS),
        ),
        healthy_successes_required: healthy_successes_required
            .unwrap_or(super::DEFAULT_HEALTHY_SUCCESSES_REQUIRED),
    }))
}

fn normalize_server_name_override(value: &str) -> String {
    let trimmed = value.trim();
    trimmed
        .strip_prefix('[')
        .and_then(|candidate| candidate.strip_suffix(']'))
        .unwrap_or(trimmed)
        .to_string()
}
