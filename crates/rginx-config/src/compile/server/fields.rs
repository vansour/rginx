use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use http::{HeaderName, HeaderValue};
use ipnet::IpNet;
use rginx_core::{AccessLogFormat, DEFAULT_SERVER_HEADER, Error, Result, Server, ServerTls};

use crate::model::ServerTlsConfig;

use super::tls::compile_server_tls;

pub(super) struct CompiledServerFields {
    pub(super) server: Server,
    pub(super) server_tls: Option<ServerTls>,
}

pub(super) struct ServerFieldConfig {
    pub(super) listen: String,
    pub(super) server_header: Option<String>,
    pub(super) default_certificate: Option<String>,
    pub(super) trusted_proxies: Vec<String>,
    pub(super) client_ip_header: Option<String>,
    pub(super) keep_alive: Option<bool>,
    pub(super) max_headers: Option<u64>,
    pub(super) max_request_body_bytes: Option<u64>,
    pub(super) max_connections: Option<u64>,
    pub(super) header_read_timeout_secs: Option<u64>,
    pub(super) request_body_read_timeout_secs: Option<u64>,
    pub(super) response_write_timeout_secs: Option<u64>,
    pub(super) access_log_format: Option<String>,
    pub(super) tls: Option<ServerTlsConfig>,
}

pub(super) fn compile_server_fields(
    config: ServerFieldConfig,
    base_dir: &Path,
) -> Result<CompiledServerFields> {
    let ServerFieldConfig {
        listen,
        server_header,
        default_certificate,
        trusted_proxies,
        client_ip_header,
        keep_alive,
        max_headers,
        max_request_body_bytes,
        max_connections,
        header_read_timeout_secs,
        request_body_read_timeout_secs,
        response_write_timeout_secs,
        access_log_format,
        tls,
    } = config;

    let server_tls = compile_server_tls(tls, base_dir)?;
    Ok(CompiledServerFields {
        server: Server {
            listen_addr: listen.parse()?,
            server_header: compile_server_header(server_header)?,
            default_certificate: compile_default_certificate(default_certificate),
            trusted_proxies: compile_trusted_proxies(trusted_proxies)?,
            client_ip_header: compile_client_ip_header(client_ip_header)?,
            keep_alive: keep_alive.unwrap_or(true),
            max_headers: compile_max_headers(max_headers)?,
            max_request_body_bytes: compile_max_request_body_bytes(max_request_body_bytes)?,
            max_connections: compile_max_connections(max_connections)?,
            header_read_timeout: header_read_timeout_secs.map(Duration::from_secs),
            request_body_read_timeout: request_body_read_timeout_secs.map(Duration::from_secs),
            response_write_timeout: response_write_timeout_secs.map(Duration::from_secs),
            access_log_format: compile_access_log_format(access_log_format)?,
            tls: server_tls.clone(),
        },
        server_tls,
    })
}

fn compile_client_ip_header(value: Option<String>) -> Result<Option<HeaderName>> {
    value
        .map(|value| {
            value.trim().parse::<HeaderName>().map_err(|error| {
                Error::Config(format!(
                    "validated server client_ip_header `{value}` failed to compile: {error}"
                ))
            })
        })
        .transpose()
}

fn compile_server_header(server_header: Option<String>) -> Result<HeaderValue> {
    let value = server_header.unwrap_or_else(|| DEFAULT_SERVER_HEADER.to_string());
    let value = value.trim();
    HeaderValue::from_str(value)
        .map_err(|error| Error::Config(format!("server_header `{value}` is invalid: {error}")))
}

fn compile_default_certificate(default_certificate: Option<String>) -> Option<String> {
    default_certificate.map(|name| name.trim().to_lowercase())
}

fn compile_max_headers(max_headers: Option<u64>) -> Result<Option<usize>> {
    max_headers
        .map(|limit| {
            usize::try_from(limit).map_err(|_| {
                Error::Config(format!("server max_headers `{limit}` exceeds platform limits"))
            })
        })
        .transpose()
}

fn compile_max_request_body_bytes(max_request_body_bytes: Option<u64>) -> Result<Option<usize>> {
    max_request_body_bytes
        .map(|limit| {
            usize::try_from(limit).map_err(|_| {
                Error::Config(format!(
                    "server max_request_body_bytes `{limit}` exceeds platform limits"
                ))
            })
        })
        .transpose()
}

fn compile_max_connections(max_connections: Option<u64>) -> Result<Option<usize>> {
    max_connections
        .map(|limit| {
            usize::try_from(limit).map_err(|_| {
                Error::Config(format!("server max_connections `{limit}` exceeds platform limits"))
            })
        })
        .transpose()
}

fn compile_access_log_format(access_log_format: Option<String>) -> Result<Option<AccessLogFormat>> {
    access_log_format.map(AccessLogFormat::parse).transpose()
}

fn compile_trusted_proxies(values: Vec<String>) -> Result<Vec<IpNet>> {
    values
        .into_iter()
        .map(|value| {
            let normalized = normalize_trusted_proxy(&value).ok_or_else(|| {
                Error::Config(format!(
                    "server trusted_proxies entry `{value}` must be a valid IP address or CIDR"
                ))
            })?;

            normalized.parse::<IpNet>().map_err(|error| {
                Error::Config(format!("server trusted_proxies entry `{value}` is invalid: {error}"))
            })
        })
        .collect()
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
