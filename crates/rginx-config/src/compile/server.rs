use std::net::IpAddr;
use std::path::Path;
use std::time::Duration;

use ipnet::IpNet;
use rginx_core::{AccessLogFormat, Error, Listener, Result, Server, ServerTls};

use crate::model::{ListenerConfig, ServerConfig, ServerTlsConfig};

pub(super) struct CompiledServer {
    pub listener: Listener,
    pub server_names: Vec<String>,
}

pub(super) fn compile_legacy_server(
    server: ServerConfig,
    base_dir: &Path,
    any_vhost_tls: bool,
) -> Result<CompiledServer> {
    let ServerConfig {
        listen,
        proxy_protocol,
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

    let listen = listen.expect("legacy server listen should be validated before compile");
    let compiled = compile_server_fields(
        ServerFieldConfig {
            listen,
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
        },
        base_dir,
    )?;

    Ok(CompiledServer {
        listener: Listener {
            id: "default".to_string(),
            name: "default".to_string(),
            server: compiled.server,
            tls_termination_enabled: compiled.server_tls.is_some() || any_vhost_tls,
            proxy_protocol_enabled: proxy_protocol.unwrap_or(false),
        },
        server_names,
    })
}

pub(super) fn compile_listeners(
    listeners: Vec<ListenerConfig>,
    base_dir: &Path,
) -> Result<Vec<Listener>> {
    listeners
        .into_iter()
        .enumerate()
        .map(|(index, listener)| {
            let ListenerConfig {
                name,
                listen,
                proxy_protocol,
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
            } = listener;

            let compiled = compile_server_fields(
                ServerFieldConfig {
                    listen,
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
                },
                base_dir,
            )?;

            Ok(Listener {
                id: format!("listeners[{index}]"),
                name,
                server: compiled.server,
                tls_termination_enabled: compiled.server_tls.is_some(),
                proxy_protocol_enabled: proxy_protocol.unwrap_or(false),
            })
        })
        .collect()
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

struct CompiledServerFields {
    server: Server,
    server_tls: Option<ServerTls>,
}

struct ServerFieldConfig {
    listen: String,
    trusted_proxies: Vec<String>,
    keep_alive: Option<bool>,
    max_headers: Option<u64>,
    max_request_body_bytes: Option<u64>,
    max_connections: Option<u64>,
    header_read_timeout_secs: Option<u64>,
    request_body_read_timeout_secs: Option<u64>,
    response_write_timeout_secs: Option<u64>,
    access_log_format: Option<String>,
    tls: Option<ServerTlsConfig>,
}

fn compile_server_fields(
    config: ServerFieldConfig,
    base_dir: &Path,
) -> Result<CompiledServerFields> {
    let ServerFieldConfig {
        listen,
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
    } = config;

    let server_tls = compile_server_tls(tls, base_dir)?;
    Ok(CompiledServerFields {
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
        server_tls,
    })
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
