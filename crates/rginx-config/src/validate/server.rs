use std::collections::HashSet;
use std::net::IpAddr;

use ipnet::IpNet;
use rginx_core::{Error, Result};

use crate::model::{
    ListenerConfig, ServerCertificateBundleConfig, ServerClientAuthConfig, ServerConfig,
    ServerTlsConfig, TlsCipherSuiteConfig, TlsKeyExchangeGroupConfig, TlsVersionConfig,
    VirtualHostConfig,
};

pub(super) fn validate_server(server: &ServerConfig) -> Result<()> {
    validate_listener_like(ListenerLikeRef {
        owner_label: "server",
        listen: server.listen.as_deref(),
        proxy_protocol: server.proxy_protocol,
        default_certificate: server.default_certificate.as_deref(),
        trusted_proxies: &server.trusted_proxies,
        max_headers: server.max_headers,
        max_request_body_bytes: server.max_request_body_bytes,
        max_connections: server.max_connections,
        header_read_timeout_secs: server.header_read_timeout_secs,
        request_body_read_timeout_secs: server.request_body_read_timeout_secs,
        response_write_timeout_secs: server.response_write_timeout_secs,
        access_log_format: server.access_log_format.as_deref(),
        tls: server.tls.as_ref(),
        require_listen: false,
    })?;

    Ok(())
}

pub(super) fn validate_listeners(
    listeners: &[ListenerConfig],
    server: &ServerConfig,
    vhosts: &[VirtualHostConfig],
) -> Result<()> {
    if listeners.is_empty() {
        if server.listen.as_deref().is_none_or(str::is_empty) {
            return Err(Error::Config(
                "server listen must be set when listeners is empty".to_string(),
            ));
        }

        return Ok(());
    }

    if legacy_server_listener_fields(server).is_some() {
        return Err(Error::Config(
            "server legacy listener fields cannot be used together with listeners".to_string(),
        ));
    }

    let mut all_listener_names = HashSet::new();
    for (index, listener) in listeners.iter().enumerate() {
        let owner = format!("listeners[{index}]");
        let normalized_name = listener.name.trim().to_lowercase();
        if normalized_name.is_empty() {
            return Err(Error::Config(format!("{owner} name must not be empty")));
        }
        if !all_listener_names.insert(normalized_name) {
            return Err(Error::Config(format!(
                "duplicate listener name `{}` across listeners",
                listener.name
            )));
        }

        validate_listener_like(ListenerLikeRef {
            owner_label: &owner,
            listen: Some(listener.listen.as_str()),
            proxy_protocol: listener.proxy_protocol,
            default_certificate: listener.default_certificate.as_deref(),
            trusted_proxies: &listener.trusted_proxies,
            max_headers: listener.max_headers,
            max_request_body_bytes: listener.max_request_body_bytes,
            max_connections: listener.max_connections,
            header_read_timeout_secs: listener.header_read_timeout_secs,
            request_body_read_timeout_secs: listener.request_body_read_timeout_secs,
            response_write_timeout_secs: listener.response_write_timeout_secs,
            access_log_format: listener.access_log_format.as_deref(),
            tls: listener.tls.as_ref(),
            require_listen: true,
        })?;
    }

    let any_listener_tls = listeners.iter().any(|listener| listener.tls.is_some());
    let any_vhost_tls = vhosts.iter().any(|vhost| vhost.tls.is_some());
    if any_vhost_tls && !any_listener_tls {
        return Err(Error::Config(
            "vhost TLS requires at least one listener with tls to be configured".to_string(),
        ));
    }

    Ok(())
}

pub(super) fn validate_server_names(
    owner_label: &str,
    server_names: &[String],
    all_server_names: &mut HashSet<String>,
) -> Result<()> {
    for name in server_names {
        let normalized = name.trim().to_lowercase();
        if normalized.is_empty() {
            return Err(Error::Config(format!("{owner_label} server_name must not be empty")));
        }

        if normalized.contains('*')
            && (!normalized.starts_with("*.")
                || normalized[2..].is_empty()
                || normalized[2..].contains('*'))
        {
            return Err(Error::Config(format!(
                "{owner_label} server_name `{name}` uses unsupported wildcard syntax; only leading `*.` patterns are supported"
            )));
        }

        if normalized.contains('/') {
            return Err(Error::Config(format!(
                "{owner_label} server_name `{name}` should not contain path separator"
            )));
        }

        if !all_server_names.insert(normalized) {
            return Err(Error::Config(format!(
                "duplicate server_name `{name}` across server and servers"
            )));
        }
    }

    Ok(())
}

struct ListenerLikeRef<'a> {
    owner_label: &'a str,
    listen: Option<&'a str>,
    proxy_protocol: Option<bool>,
    default_certificate: Option<&'a str>,
    trusted_proxies: &'a [String],
    max_headers: Option<u64>,
    max_request_body_bytes: Option<u64>,
    max_connections: Option<u64>,
    header_read_timeout_secs: Option<u64>,
    request_body_read_timeout_secs: Option<u64>,
    response_write_timeout_secs: Option<u64>,
    access_log_format: Option<&'a str>,
    tls: Option<&'a ServerTlsConfig>,
    require_listen: bool,
}

fn validate_listener_like(config: ListenerLikeRef<'_>) -> Result<()> {
    if config.require_listen {
        let listen = config.listen.unwrap_or_default().trim();
        if listen.is_empty() {
            return Err(Error::Config(format!("{} listen must not be empty", config.owner_label)));
        }
    }

    let _ = config.proxy_protocol;

    if config.default_certificate.is_some_and(|value| value.trim().is_empty()) {
        return Err(Error::Config(format!(
            "{} default_certificate must not be empty",
            config.owner_label
        )));
    }

    for value in config.trusted_proxies {
        validate_trusted_proxy_with_owner(config.owner_label, value)?;
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

    if let Some(ServerTlsConfig {
        cert_path,
        key_path,
        additional_certificates,
        versions,
        cipher_suites,
        key_exchange_groups,
        alpn_protocols,
        ocsp_staple_path,
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

pub(super) fn validate_tls_identity_fields(
    owner_label: &str,
    cert_path: &str,
    key_path: &str,
    additional_certificates: Option<&[ServerCertificateBundleConfig]>,
    ocsp_staple_path: Option<&str>,
) -> Result<()> {
    if cert_path.trim().is_empty() {
        return Err(Error::Config(format!("{owner_label} TLS certificate path must not be empty")));
    }

    if key_path.trim().is_empty() {
        return Err(Error::Config(format!("{owner_label} TLS private key path must not be empty")));
    }

    if ocsp_staple_path.is_some_and(|path| path.trim().is_empty()) {
        return Err(Error::Config(format!("{owner_label} TLS OCSP staple path must not be empty")));
    }

    if let Some(additional_certificates) = additional_certificates {
        if additional_certificates.is_empty() {
            return Err(Error::Config(format!(
                "{owner_label} TLS additional_certificates must not be empty"
            )));
        }

        for bundle in additional_certificates {
            validate_certificate_bundle(owner_label, bundle)?;
        }
    }

    Ok(())
}

fn validate_certificate_bundle(
    owner_label: &str,
    bundle: &ServerCertificateBundleConfig,
) -> Result<()> {
    if bundle.cert_path.trim().is_empty() {
        return Err(Error::Config(format!(
            "{owner_label} TLS additional certificate path must not be empty"
        )));
    }

    if bundle.key_path.trim().is_empty() {
        return Err(Error::Config(format!(
            "{owner_label} TLS additional private key path must not be empty"
        )));
    }

    if bundle.ocsp_staple_path.as_ref().is_some_and(|path| path.trim().is_empty()) {
        return Err(Error::Config(format!(
            "{owner_label} TLS additional OCSP staple path must not be empty"
        )));
    }

    Ok(())
}

fn validate_tls_versions(owner_label: &str, versions: Option<&[TlsVersionConfig]>) -> Result<()> {
    let Some(versions) = versions else {
        return Ok(());
    };

    if versions.is_empty() {
        return Err(Error::Config(format!("{owner_label} TLS versions must not be empty")));
    }

    let mut seen = HashSet::new();
    for version in versions {
        if !seen.insert(version) {
            return Err(Error::Config(format!(
                "{owner_label} TLS versions must not contain duplicates"
            )));
        }
    }

    Ok(())
}

fn validate_tls_cipher_suites(
    owner_label: &str,
    cipher_suites: Option<&[TlsCipherSuiteConfig]>,
    versions: Option<&[TlsVersionConfig]>,
) -> Result<()> {
    let Some(cipher_suites) = cipher_suites else {
        return Ok(());
    };

    if cipher_suites.is_empty() {
        return Err(Error::Config(format!("{owner_label} TLS cipher_suites must not be empty")));
    }

    let mut seen = HashSet::new();
    for suite in cipher_suites {
        if !seen.insert(*suite) {
            return Err(Error::Config(format!(
                "{owner_label} TLS cipher_suites must not contain duplicates"
            )));
        }
    }

    if let Some(versions) = versions
        && !cipher_suites.iter().any(|suite| {
            versions.iter().any(|version| cipher_suite_supports_version(*suite, *version))
        })
    {
        return Err(Error::Config(format!(
            "{owner_label} TLS cipher_suites do not support any configured TLS versions"
        )));
    }

    Ok(())
}

fn validate_tls_key_exchange_groups(
    owner_label: &str,
    key_exchange_groups: Option<&[TlsKeyExchangeGroupConfig]>,
) -> Result<()> {
    let Some(key_exchange_groups) = key_exchange_groups else {
        return Ok(());
    };

    if key_exchange_groups.is_empty() {
        return Err(Error::Config(format!(
            "{owner_label} TLS key_exchange_groups must not be empty"
        )));
    }

    let mut seen = HashSet::new();
    for group in key_exchange_groups {
        if !seen.insert(*group) {
            return Err(Error::Config(format!(
                "{owner_label} TLS key_exchange_groups must not contain duplicates"
            )));
        }
    }

    Ok(())
}

fn cipher_suite_supports_version(suite: TlsCipherSuiteConfig, version: TlsVersionConfig) -> bool {
    match suite {
        TlsCipherSuiteConfig::Tls13Aes256GcmSha384
        | TlsCipherSuiteConfig::Tls13Aes128GcmSha256
        | TlsCipherSuiteConfig::Tls13Chacha20Poly1305Sha256 => version == TlsVersionConfig::Tls13,
        TlsCipherSuiteConfig::TlsEcdheEcdsaWithAes256GcmSha384
        | TlsCipherSuiteConfig::TlsEcdheEcdsaWithAes128GcmSha256
        | TlsCipherSuiteConfig::TlsEcdheEcdsaWithChacha20Poly1305Sha256
        | TlsCipherSuiteConfig::TlsEcdheRsaWithAes256GcmSha384
        | TlsCipherSuiteConfig::TlsEcdheRsaWithAes128GcmSha256
        | TlsCipherSuiteConfig::TlsEcdheRsaWithChacha20Poly1305Sha256 => {
            version == TlsVersionConfig::Tls12
        }
    }
}

fn validate_alpn_protocols(owner_label: &str, alpn_protocols: Option<&[String]>) -> Result<()> {
    let Some(alpn_protocols) = alpn_protocols else {
        return Ok(());
    };

    if alpn_protocols.is_empty() {
        return Err(Error::Config(format!(
            "{owner_label} TLS ALPN protocol list must not be empty"
        )));
    }

    let mut seen = HashSet::new();
    for protocol in alpn_protocols {
        let normalized = protocol.trim();
        if normalized.is_empty() {
            return Err(Error::Config(format!(
                "{owner_label} TLS ALPN protocol entries must not be empty"
            )));
        }

        if !seen.insert(normalized.to_ascii_lowercase()) {
            return Err(Error::Config(format!(
                "{owner_label} TLS ALPN protocol list must not contain duplicates"
            )));
        }
    }

    Ok(())
}

fn legacy_server_listener_fields(server: &ServerConfig) -> Option<()> {
    (server.listen.is_some()
        || !server.trusted_proxies.is_empty()
        || server.keep_alive.is_some()
        || server.default_certificate.is_some()
        || server.max_headers.is_some()
        || server.max_request_body_bytes.is_some()
        || server.max_connections.is_some()
        || server.header_read_timeout_secs.is_some()
        || server.request_body_read_timeout_secs.is_some()
        || server.response_write_timeout_secs.is_some()
        || server.access_log_format.is_some()
        || server.tls.is_some())
    .then_some(())
}

fn validate_trusted_proxy_with_owner(owner_label: &str, value: &str) -> Result<()> {
    let normalized = normalize_trusted_proxy(value).ok_or_else(|| {
        Error::Config(format!(
            "{owner_label} trusted_proxies entry `{value}` must be a valid IP address or CIDR"
        ))
    })?;

    normalized.parse::<IpNet>().map_err(|error| {
        Error::Config(format!("{owner_label} trusted_proxies entry `{value}` is invalid: {error}"))
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
