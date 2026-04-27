use http::uri::PathAndQuery;

use super::*;

pub(super) fn validate_active_health_settings(upstream: &UpstreamConfig) -> Result<()> {
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
            && path != super::super::DEFAULT_GRPC_HEALTH_CHECK_PATH
        {
            return Err(Error::Config(format!(
                "upstream `{}` health_check_grpc_service requires health_check_path to be `{}`",
                upstream.name,
                super::super::DEFAULT_GRPC_HEALTH_CHECK_PATH
            )));
        }

        if matches!(upstream.protocol, UpstreamProtocolConfig::Http1) {
            return Err(Error::Config(format!(
                "upstream `{}` health_check_grpc_service requires protocol `Auto`, `Http2`, or `H2c`",
                upstream.name
            )));
        }

        if matches!(upstream.protocol, UpstreamProtocolConfig::H2c) {
            if upstream.peers.iter().any(|peer| !peer.url.starts_with("http://")) {
                return Err(Error::Config(format!(
                    "upstream `{}` health_check_grpc_service with protocol `H2c` requires all peers to use `http://`",
                    upstream.name
                )));
            }
        } else if upstream.peers.iter().any(|peer| !peer.url.starts_with("https://")) {
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

    for (field, value) in [
        ("health_check_interval_secs", upstream.health_check_interval_secs),
        ("health_check_timeout_secs", upstream.health_check_timeout_secs),
    ] {
        if value.is_some_and(|value| value == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` {field} must be greater than 0",
                upstream.name
            )));
        }
    }

    if upstream.healthy_successes_required.is_some_and(|value| value == 0) {
        return Err(Error::Config(format!(
            "upstream `{}` healthy_successes_required must be greater than 0",
            upstream.name
        )));
    }

    Ok(())
}
