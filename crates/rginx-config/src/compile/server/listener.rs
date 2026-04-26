use std::path::Path;

use rginx_core::{Listener, Result};

use crate::model::{ListenerConfig, ServerConfig};

use super::CompiledServer;
use super::fields::{ServerFieldConfig, compile_server_fields};
use super::http3::compile_http3;

pub(super) fn compile_legacy_server(
    server: ServerConfig,
    base_dir: &Path,
    any_vhost_tls: bool,
) -> Result<CompiledServer> {
    let ServerConfig {
        listen,
        server_header,
        proxy_protocol,
        default_certificate,
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
        http3,
    } = server;

    let listen = listen.expect("legacy server listen should be validated before compile");
    let compiled = compile_server_fields(
        ServerFieldConfig {
            listen,
            server_header,
            default_certificate,
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
    let http3 = compile_http3(
        http3,
        compiled.server.listen_addr,
        compiled.server_tls.is_some() || any_vhost_tls,
        base_dir,
    )?;

    Ok(CompiledServer {
        listener: Listener {
            id: "default".to_string(),
            name: "default".to_string(),
            server: compiled.server,
            tls_termination_enabled: compiled.server_tls.is_some() || any_vhost_tls,
            proxy_protocol_enabled: proxy_protocol.unwrap_or(false),
            http3,
        },
        server_names,
    })
}

pub(super) fn compile_listeners(
    listeners: Vec<ListenerConfig>,
    default_server_header: Option<String>,
    base_dir: &Path,
) -> Result<Vec<Listener>> {
    listeners
        .into_iter()
        .map(|listener| {
            let ListenerConfig {
                name,
                listen,
                server_header,
                proxy_protocol,
                default_certificate,
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
                http3,
            } = listener;

            let compiled = compile_server_fields(
                ServerFieldConfig {
                    listen,
                    server_header: server_header.or_else(|| default_server_header.clone()),
                    default_certificate,
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
            let http3 = compile_http3(
                http3,
                compiled.server.listen_addr,
                compiled.server_tls.is_some(),
                base_dir,
            )?;

            Ok(Listener {
                id: explicit_listener_id(&name),
                name,
                server: compiled.server,
                tls_termination_enabled: compiled.server_tls.is_some(),
                proxy_protocol_enabled: proxy_protocol.unwrap_or(false),
                http3,
            })
        })
        .collect()
}

fn explicit_listener_id(name: &str) -> String {
    format!("listener:{}", name.trim().to_ascii_lowercase())
}
