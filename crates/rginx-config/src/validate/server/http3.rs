use rginx_core::{Error, Result};

use crate::model::{Http3Config, ServerTlsConfig, TlsVersionConfig};

/// Validates HTTP/3-specific configuration and TLS compatibility requirements.
pub(super) fn validate_http3(
    owner_label: &str,
    http3: &Http3Config,
    tls: Option<&ServerTlsConfig>,
) -> Result<()> {
    let Some(tls) = tls else {
        return Err(Error::Config(format!(
            "{owner_label} http3 requires tls to be configured on the same listener"
        )));
    };

    if http3.listen.as_deref().is_some_and(|listen| listen.trim().is_empty()) {
        return Err(Error::Config(format!(
            "{owner_label} http3 listen must not be empty when provided"
        )));
    }

    if http3.alt_svc_max_age_secs.is_some_and(|value| value == 0) {
        return Err(Error::Config(format!(
            "{owner_label} http3 alt_svc_max_age_secs must be greater than 0"
        )));
    }

    if http3.max_concurrent_streams.is_some_and(|value| value == 0) {
        return Err(Error::Config(format!(
            "{owner_label} http3 max_concurrent_streams must be greater than 0"
        )));
    }

    if http3.stream_buffer_size_bytes.is_some_and(|value| value == 0) {
        return Err(Error::Config(format!(
            "{owner_label} http3 stream_buffer_size_bytes must be greater than 0"
        )));
    }

    if http3.active_connection_id_limit.is_some_and(|value| value < 2) {
        return Err(Error::Config(format!(
            "{owner_label} http3 active_connection_id_limit must be greater than or equal to 2"
        )));
    }

    if let Some(limit) = http3.active_connection_id_limit
        && !matches!(limit, 2 | 5)
    {
        return Err(Error::Config(format!(
            "{owner_label} http3 active_connection_id_limit currently supports only 2 or 5 with the active QUIC stack"
        )));
    }

    if http3.host_key_path.as_ref().is_some_and(|path| path.trim().is_empty()) {
        return Err(Error::Config(format!("{owner_label} http3 host_key_path must not be empty")));
    }

    if matches!(http3.retry, Some(true)) && http3.host_key_path.is_none() {
        return Err(Error::Config(format!(
            "{owner_label} http3 retry requires host_key_path to be configured"
        )));
    }

    if let Some(versions) = tls.versions.as_deref()
        && !versions.contains(&TlsVersionConfig::Tls13)
    {
        return Err(Error::Config(format!(
            "{owner_label} http3 requires TLS1.3 to remain enabled on the same listener"
        )));
    }

    if matches!(http3.early_data, Some(true)) {
        if matches!(tls.session_resumption, Some(false)) {
            return Err(Error::Config(format!(
                "{owner_label} http3 early_data requires tls session_resumption to remain enabled"
            )));
        }

        if tls.session_cache_size.is_some_and(|size| size == 0) {
            return Err(Error::Config(format!(
                "{owner_label} http3 early_data requires tls session_cache_size to remain greater than 0"
            )));
        }

        if matches!(tls.session_tickets, Some(true)) {
            return Err(Error::Config(format!(
                "{owner_label} http3 early_data requires tls session_tickets to remain disabled so stateful resumption is used"
            )));
        }
    }

    const HTTP3_ACTIVE_CONNECTION_ID_LIMIT_MIGRATION: u32 = 5;
    if matches!(http3.active_connection_id_limit, Some(HTTP3_ACTIVE_CONNECTION_ID_LIMIT_MIGRATION))
        && http3.host_key_path.is_none()
    {
        return Err(Error::Config(format!(
            "{owner_label} http3 active_connection_id_limit=5 requires host_key_path to be configured"
        )));
    }

    Ok(())
}
