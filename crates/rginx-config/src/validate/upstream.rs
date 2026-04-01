use std::collections::HashSet;

use http::uri::PathAndQuery;
use rginx_core::{Error, Result};

use crate::model::{UpstreamConfig, UpstreamProtocolConfig, UpstreamTlsConfig};

pub(super) fn validate_upstreams(upstreams: &[UpstreamConfig]) -> Result<HashSet<String>> {
    let mut upstream_names = HashSet::new();

    for upstream in upstreams {
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
                let uri = peer.url.parse::<http::Uri>().map_err(|error| {
                    Error::Config(format!(
                        "upstream `{}` peer url `{}` is not a valid URI: {error}",
                        upstream.name, peer.url
                    ))
                })?;

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
                && path != super::DEFAULT_GRPC_HEALTH_CHECK_PATH
            {
                return Err(Error::Config(format!(
                    "upstream `{}` health_check_grpc_service requires health_check_path to be `{}`",
                    upstream.name,
                    super::DEFAULT_GRPC_HEALTH_CHECK_PATH
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

    Ok(upstream_names)
}
