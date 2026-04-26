use super::*;

pub(super) fn validate_timeout_and_tuning(upstream: &UpstreamConfig) -> Result<()> {
    for (field, value) in [
        ("request_timeout_secs", upstream.request_timeout_secs),
        ("connect_timeout_secs", upstream.connect_timeout_secs),
        ("read_timeout_secs", upstream.read_timeout_secs),
        ("write_timeout_secs", upstream.write_timeout_secs),
        ("idle_timeout_secs", upstream.idle_timeout_secs),
        ("tcp_keepalive_secs", upstream.tcp_keepalive_secs),
        ("http2_keep_alive_interval_secs", upstream.http2_keep_alive_interval_secs),
        ("http2_keep_alive_timeout_secs", upstream.http2_keep_alive_timeout_secs),
    ] {
        if value.is_some_and(|value| value == 0) {
            return Err(Error::Config(format!(
                "upstream `{}` {field} must be greater than 0",
                upstream.name
            )));
        }
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

    Ok(())
}
