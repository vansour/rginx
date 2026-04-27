use rginx_core::{Error, Result};

use crate::model::{Http3Config, ServerClientAuthConfig, ServerConfig, ServerTlsConfig};

use super::super::http3::validate_http3;
use super::super::proxies::validate_trusted_proxy_with_owner;
use super::super::tls::{
    validate_alpn_protocols, validate_tls_cipher_suites, validate_tls_identity_fields,
    validate_tls_key_exchange_groups, validate_tls_versions,
};

pub(super) fn validate_server(server: &ServerConfig) -> Result<()> {
    validate_listener_like(ListenerLikeRef {
        owner_label: "server",
        listen: server.listen.as_deref(),
        server_header: server.server_header.as_deref(),
        proxy_protocol: server.proxy_protocol,
        default_certificate: server.default_certificate.as_deref(),
        trusted_proxies: &server.trusted_proxies,
        client_ip_header: server.client_ip_header.as_deref(),
        max_headers: server.max_headers,
        max_request_body_bytes: server.max_request_body_bytes,
        max_connections: server.max_connections,
        header_read_timeout_secs: server.header_read_timeout_secs,
        request_body_read_timeout_secs: server.request_body_read_timeout_secs,
        response_write_timeout_secs: server.response_write_timeout_secs,
        access_log_format: server.access_log_format.as_deref(),
        tls: server.tls.as_ref(),
        http3: server.http3.as_ref(),
        require_listen: false,
    })?;

    Ok(())
}

pub(super) struct ListenerLikeRef<'a> {
    pub(super) owner_label: &'a str,
    pub(super) listen: Option<&'a str>,
    pub(super) server_header: Option<&'a str>,
    pub(super) proxy_protocol: Option<bool>,
    pub(super) default_certificate: Option<&'a str>,
    pub(super) trusted_proxies: &'a [String],
    pub(super) client_ip_header: Option<&'a str>,
    pub(super) max_headers: Option<u64>,
    pub(super) max_request_body_bytes: Option<u64>,
    pub(super) max_connections: Option<u64>,
    pub(super) header_read_timeout_secs: Option<u64>,
    pub(super) request_body_read_timeout_secs: Option<u64>,
    pub(super) response_write_timeout_secs: Option<u64>,
    pub(super) access_log_format: Option<&'a str>,
    pub(super) tls: Option<&'a ServerTlsConfig>,
    pub(super) http3: Option<&'a Http3Config>,
    pub(super) require_listen: bool,
}

pub(super) fn validate_listener_like(config: ListenerLikeRef<'_>) -> Result<()> {
    if config.require_listen {
        let listen = config.listen.unwrap_or_default().trim();
        if listen.is_empty() {
            return Err(Error::Config(format!("{} listen must not be empty", config.owner_label)));
        }
    }

    let _ = config.proxy_protocol;

    if let Some(server_header) = config.server_header {
        validate_server_header(config.owner_label, server_header)?;
    }

    if config.default_certificate.is_some_and(|value| value.trim().is_empty()) {
        return Err(Error::Config(format!(
            "{} default_certificate must not be empty",
            config.owner_label
        )));
    }

    for value in config.trusted_proxies {
        validate_trusted_proxy_with_owner(config.owner_label, value)?;
    }

    if let Some(client_ip_header) = config.client_ip_header {
        let normalized = client_ip_header.trim();
        if normalized.is_empty() {
            return Err(Error::Config(format!(
                "{} client_ip_header must not be empty",
                config.owner_label
            )));
        }
        normalized.parse::<http::header::HeaderName>().map_err(|error| {
            Error::Config(format!(
                "{} client_ip_header `{client_ip_header}` is invalid: {error}",
                config.owner_label
            ))
        })?;
    }

    if config.max_headers.is_some_and(|limit| limit == 0) {
        return Err(Error::Config(format!(
            "{} max_headers must be greater than 0",
            config.owner_label
        )));
    }

    if config.max_request_body_bytes.is_some_and(|limit| limit == 0) {
        return Err(Error::Config(format!(
            "{} max_request_body_bytes must be greater than 0",
            config.owner_label
        )));
    }

    if config.max_connections.is_some_and(|limit| limit == 0) {
        return Err(Error::Config(format!(
            "{} max_connections must be greater than 0",
            config.owner_label
        )));
    }

    if config.header_read_timeout_secs.is_some_and(|timeout| timeout == 0) {
        return Err(Error::Config(format!(
            "{} header_read_timeout_secs must be greater than 0",
            config.owner_label
        )));
    }

    if config.request_body_read_timeout_secs.is_some_and(|timeout| timeout == 0) {
        return Err(Error::Config(format!(
            "{} request_body_read_timeout_secs must be greater than 0",
            config.owner_label
        )));
    }

    if config.response_write_timeout_secs.is_some_and(|timeout| timeout == 0) {
        return Err(Error::Config(format!(
            "{} response_write_timeout_secs must be greater than 0",
            config.owner_label
        )));
    }

    if config.access_log_format.is_some_and(|format| format.trim().is_empty()) {
        return Err(Error::Config(format!(
            "{} access_log_format must not be empty",
            config.owner_label
        )));
    }

    if let Some(http3) = config.http3 {
        validate_http3(config.owner_label, http3, config.tls)?;
    }

    if let Some(ServerTlsConfig {
        cert_path,
        key_path,
        additional_certificates,
        versions,
        cipher_suites,
        key_exchange_groups,
        alpn_protocols,
        ocsp_staple_path,
        ocsp: _,
        session_resumption,
        session_tickets,
        session_cache_size,
        session_ticket_count,
        client_auth,
    }) = config.tls
    {
        validate_tls_identity_fields(
            config.owner_label,
            cert_path,
            key_path,
            additional_certificates.as_deref(),
            ocsp_staple_path.as_deref(),
        )?;

        validate_tls_versions(config.owner_label, versions.as_deref())?;
        validate_tls_cipher_suites(
            config.owner_label,
            cipher_suites.as_deref(),
            versions.as_deref(),
        )?;
        validate_tls_key_exchange_groups(config.owner_label, key_exchange_groups.as_deref())?;
        validate_alpn_protocols(config.owner_label, alpn_protocols.as_deref())?;

        if matches!(session_resumption, Some(false)) && matches!(session_tickets, Some(true)) {
            return Err(Error::Config(format!(
                "{} TLS session_tickets requires session_resumption to remain enabled",
                config.owner_label
            )));
        }

        if matches!(session_resumption, Some(false)) && session_cache_size.is_some() {
            return Err(Error::Config(format!(
                "{} TLS session_cache_size cannot be set when session_resumption is disabled",
                config.owner_label
            )));
        }

        if matches!(session_resumption, Some(false)) && session_ticket_count.is_some() {
            return Err(Error::Config(format!(
                "{} TLS session_ticket_count cannot be set when session_resumption is disabled",
                config.owner_label
            )));
        }

        if matches!(session_tickets, Some(false)) && session_ticket_count.is_some() {
            return Err(Error::Config(format!(
                "{} TLS session_ticket_count cannot be set when session_tickets is disabled",
                config.owner_label
            )));
        }

        if session_ticket_count.is_some_and(|count| count == 0) {
            return Err(Error::Config(format!(
                "{} TLS session_ticket_count must be greater than 0",
                config.owner_label
            )));
        }

        if let Some(ServerClientAuthConfig { ca_cert_path, verify_depth, crl_path, .. }) =
            client_auth
        {
            if ca_cert_path.trim().is_empty() {
                return Err(Error::Config(format!(
                    "{} TLS client auth CA path must not be empty",
                    config.owner_label
                )));
            }

            if verify_depth.is_some_and(|depth| depth == 0) {
                return Err(Error::Config(format!(
                    "{} TLS client auth verify_depth must be greater than 0",
                    config.owner_label
                )));
            }

            if crl_path.as_ref().is_some_and(|path| path.trim().is_empty()) {
                return Err(Error::Config(format!(
                    "{} TLS client auth CRL path must not be empty",
                    config.owner_label
                )));
            }
        }
    }

    Ok(())
}

pub(super) fn legacy_server_listener_fields(server: &ServerConfig) -> Option<()> {
    (server.listen.is_some()
        || !server.trusted_proxies.is_empty()
        || server.client_ip_header.is_some()
        || server.keep_alive.is_some()
        || server.default_certificate.is_some()
        || server.max_headers.is_some()
        || server.max_request_body_bytes.is_some()
        || server.max_connections.is_some()
        || server.header_read_timeout_secs.is_some()
        || server.request_body_read_timeout_secs.is_some()
        || server.response_write_timeout_secs.is_some()
        || server.access_log_format.is_some()
        || server.tls.is_some()
        || server.http3.is_some())
    .then_some(())
}

fn validate_server_header(owner_label: &str, server_header: &str) -> Result<()> {
    let value = server_header.trim();
    if value.is_empty() {
        return Err(Error::Config(format!("{owner_label} server_header must not be empty")));
    }

    http::HeaderValue::from_str(value).map_err(|error| {
        Error::Config(format!("{owner_label} server_header `{server_header}` is invalid: {error}"))
    })?;

    Ok(())
}
