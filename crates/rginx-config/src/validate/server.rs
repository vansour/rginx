use std::collections::HashSet;
use std::net::IpAddr;

use ipnet::IpNet;
use rginx_core::{Error, Result};

use crate::model::{ServerConfig, ServerTlsConfig};

pub(super) fn validate_server(server: &ServerConfig) -> Result<()> {
    for value in &server.trusted_proxies {
        validate_trusted_proxy(value)?;
    }

    if server.max_headers.is_some_and(|limit| limit == 0) {
        return Err(Error::Config("server max_headers must be greater than 0".to_string()));
    }

    if server.max_request_body_bytes.is_some_and(|limit| limit == 0) {
        return Err(Error::Config(
            "server max_request_body_bytes must be greater than 0".to_string(),
        ));
    }

    if server.max_connections.is_some_and(|limit| limit == 0) {
        return Err(Error::Config("server max_connections must be greater than 0".to_string()));
    }

    if server.header_read_timeout_secs.is_some_and(|timeout| timeout == 0) {
        return Err(Error::Config(
            "server header_read_timeout_secs must be greater than 0".to_string(),
        ));
    }

    if server.request_body_read_timeout_secs.is_some_and(|timeout| timeout == 0) {
        return Err(Error::Config(
            "server request_body_read_timeout_secs must be greater than 0".to_string(),
        ));
    }

    if server.response_write_timeout_secs.is_some_and(|timeout| timeout == 0) {
        return Err(Error::Config(
            "server response_write_timeout_secs must be greater than 0".to_string(),
        ));
    }

    if server.access_log_format.as_deref().is_some_and(|format| format.trim().is_empty()) {
        return Err(Error::Config("server access_log_format must not be empty".to_string()));
    }

    if server
        .config_api_token
        .as_deref()
        .is_some_and(|token| token.trim().is_empty() || token.trim() != token)
    {
        return Err(Error::Config(
            "server config_api_token must not be empty or contain leading/trailing whitespace"
                .to_string(),
        ));
    }

    if let Some(ServerTlsConfig { cert_path, key_path }) = &server.tls {
        if cert_path.trim().is_empty() {
            return Err(Error::Config("server TLS certificate path must not be empty".to_string()));
        }

        if key_path.trim().is_empty() {
            return Err(Error::Config("server TLS private key path must not be empty".to_string()));
        }
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

fn validate_trusted_proxy(value: &str) -> Result<()> {
    let normalized = normalize_trusted_proxy(value).ok_or_else(|| {
        Error::Config(format!(
            "server trusted_proxies entry `{value}` must be a valid IP address or CIDR"
        ))
    })?;

    normalized.parse::<IpNet>().map_err(|error| {
        Error::Config(format!("server trusted_proxies entry `{value}` is invalid: {error}"))
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
