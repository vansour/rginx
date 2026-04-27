use std::str::FromStr;
use std::time::Duration;

use rginx_core::{
    ActiveHealthCheck, Error, Result, UpstreamLoadBalance, UpstreamPeer, UpstreamProtocol,
};
use rustls::pki_types::ServerName;

use crate::model::{UpstreamLoadBalanceConfig, UpstreamProtocolConfig};

pub(super) fn compile_timeout_secs(raw: u64, upstream_name: &str, field: &str) -> Result<Duration> {
    if raw == 0 {
        return Err(Error::Config(format!(
            "upstream `{upstream_name}` {field} must be greater than 0"
        )));
    }

    Ok(Duration::from_secs(raw))
}

pub(super) fn compile_pool_max_idle_per_host(upstream_name: &str, raw: u64) -> Result<usize> {
    usize::try_from(raw).map_err(|_| {
        Error::Config(format!(
            "upstream `{upstream_name}` pool_max_idle_per_host `{raw}` exceeds platform limits"
        ))
    })
}

pub(super) fn compile_protocol(
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
        UpstreamProtocolConfig::H2c => {
            if peers.iter().any(|peer| peer.scheme != "http") {
                return Err(Error::Config(format!(
                    "upstream `{upstream_name}` protocol `H2c` requires all peers to use `http://`"
                )));
            }

            Ok(UpstreamProtocol::H2c)
        }
        UpstreamProtocolConfig::Http3 => {
            if peers.iter().any(|peer| peer.scheme != "https") {
                return Err(Error::Config(format!(
                    "upstream `{upstream_name}` protocol `Http3` currently requires all peers to use `https://`; cleartext upstreams are not supported"
                )));
            }

            Ok(UpstreamProtocol::Http3)
        }
    }
}

pub(super) fn compile_load_balance(load_balance: UpstreamLoadBalanceConfig) -> UpstreamLoadBalance {
    match load_balance {
        UpstreamLoadBalanceConfig::RoundRobin => UpstreamLoadBalance::RoundRobin,
        UpstreamLoadBalanceConfig::IpHash => UpstreamLoadBalance::IpHash,
        UpstreamLoadBalanceConfig::LeastConn => UpstreamLoadBalance::LeastConn,
    }
}

pub(super) fn compile_server_name_override(
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

pub(super) fn compile_max_replayable_request_body_bytes(
    upstream_name: &str,
    max_replayable_request_body_bytes: Option<u64>,
) -> Result<usize> {
    let bytes = max_replayable_request_body_bytes
        .unwrap_or(super::super::DEFAULT_MAX_REPLAYABLE_REQUEST_BODY_BYTES);
    usize::try_from(bytes).map_err(|_| {
        Error::Config(format!(
            "upstream `{upstream_name}` max_replayable_request_body_bytes `{bytes}` exceeds platform limits"
        ))
    })
}

pub(super) fn compile_active_health_check(
    upstream_name: &str,
    health_check_path: Option<String>,
    health_check_grpc_service: Option<String>,
    health_check_interval_secs: Option<u64>,
    health_check_timeout_secs: Option<u64>,
    healthy_successes_required: Option<u32>,
) -> Result<Option<ActiveHealthCheck>> {
    let path = match (health_check_path, health_check_grpc_service.as_ref()) {
        (Some(path), _) => path,
        (None, Some(_)) => super::super::DEFAULT_GRPC_HEALTH_CHECK_PATH.to_string(),
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
            health_check_interval_secs.unwrap_or(super::super::DEFAULT_HEALTH_CHECK_INTERVAL_SECS),
        ),
        timeout: Duration::from_secs(
            health_check_timeout_secs.unwrap_or(super::super::DEFAULT_HEALTH_CHECK_TIMEOUT_SECS),
        ),
        healthy_successes_required: healthy_successes_required
            .unwrap_or(super::super::DEFAULT_HEALTHY_SUCCESSES_REQUIRED),
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
