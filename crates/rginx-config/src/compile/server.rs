use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use ipnet::IpNet;
use rginx_core::{AccessLogFormat, Error, Result, Server, ServerTls};

use crate::model::{ServerConfig, ServerTlsConfig};

pub(super) struct CompiledServer {
    pub server: Server,
    pub server_names: Vec<String>,
    pub server_tls: Option<ServerTls>,
}

pub(super) fn compile_server(server: ServerConfig, base_dir: &Path) -> Result<CompiledServer> {
    let ServerConfig {
        listen,
        server_names,
        trusted_proxies,
        keep_alive,
        max_headers,
        max_request_body_bytes,
        max_connections,
        header_read_timeout_secs,
        request_body_read_timeout_secs,
        response_write_timeout_secs,
        access_log_format,
        tls,
    } = server;

    let server_tls = compile_server_tls(tls, base_dir)?;
    Ok(CompiledServer {
        server: Server {
            listen_addr: listen.parse()?,
            trusted_proxies: compile_trusted_proxies(trusted_proxies)?,
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
        server_names,
        server_tls,
    })
}

pub(super) fn compile_server_tls(
    tls: Option<ServerTlsConfig>,
    base_dir: &Path,
) -> Result<Option<ServerTls>> {
    let Some(ServerTlsConfig { cert_path, key_path }) = tls else {
        return Ok(None);
    };

    let cert_path = super::resolve_path(base_dir, cert_path);
    if !cert_path.is_file() {
        return Err(Error::Config(format!(
            "server TLS certificate file `{}` does not exist or is not a file",
            cert_path.display()
        )));
    }

    let key_path = super::resolve_path(base_dir, key_path);
    if !key_path.is_file() {
        return Err(Error::Config(format!(
            "server TLS private key file `{}` does not exist or is not a file",
            key_path.display()
        )));
    }

    Ok(Some(ServerTls { cert_path, key_path }))
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
