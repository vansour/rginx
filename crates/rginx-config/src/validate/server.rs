use std::collections::HashSet;
use std::net::IpAddr;

use ipnet::IpNet;
use rginx_core::{Error, Result};

use crate::model::{ListenerConfig, ServerConfig, ServerTlsConfig, VirtualHostConfig};

pub(super) fn validate_server(server: &ServerConfig) -> Result<()> {
    validate_listener_like(ListenerLikeRef {
        owner_label: "server",
        listen: server.listen.as_deref(),
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

    if let Some(ServerTlsConfig { cert_path, key_path }) = config.tls {
        if cert_path.trim().is_empty() {
            return Err(Error::Config(format!(
                "{} TLS certificate path must not be empty",
                config.owner_label
            )));
        }

        if key_path.trim().is_empty() {
            return Err(Error::Config(format!(
                "{} TLS private key path must not be empty",
                config.owner_label
            )));
        }
    }

    Ok(())
}

fn legacy_server_listener_fields(server: &ServerConfig) -> Option<()> {
    (server.listen.is_some()
        || !server.trusted_proxies.is_empty()
        || server.keep_alive.is_some()
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
