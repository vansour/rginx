use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::Path;

use rginx_core::{Listener, Result};

use crate::model::{Http3Config, ListenerConfig, ServerConfig, VirtualHostConfig};

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

pub(super) fn compile_vhost_listeners(
    vhosts: &[VirtualHostConfig],
    server_defaults: &ServerConfig,
    base_dir: &Path,
) -> Result<Vec<Listener>> {
    let mut bindings = BTreeMap::<SocketAddr, VhostListenerBinding>::new();

    for (vhost_index, vhost) in vhosts.iter().enumerate() {
        for (listen_index, listen) in vhost.listen.iter().enumerate() {
            let owner = format!("servers[{vhost_index}].listen[{listen_index}]");
            let parsed = crate::listen::parse_vhost_listen(&owner, listen)?;
            bindings
                .entry(parsed.addr)
                .and_modify(|binding| {
                    if parsed.http3 && binding.http3.is_none() {
                        binding.http3 = Some(vhost.http3.clone().unwrap_or_default());
                    }
                })
                .or_insert(VhostListenerBinding {
                    ssl: parsed.ssl,
                    http3: parsed.http3.then(|| vhost.http3.clone().unwrap_or_default()),
                    proxy_protocol: parsed.proxy_protocol,
                });
        }
    }

    bindings
        .into_iter()
        .map(|(listen_addr, binding)| {
            let compiled = compile_server_fields(
                ServerFieldConfig {
                    listen: listen_addr.to_string(),
                    server_header: server_defaults.server_header.clone(),
                    default_certificate: None,
                    trusted_proxies: server_defaults.trusted_proxies.clone(),
                    keep_alive: server_defaults.keep_alive,
                    max_headers: server_defaults.max_headers,
                    max_request_body_bytes: server_defaults.max_request_body_bytes,
                    max_connections: server_defaults.max_connections,
                    header_read_timeout_secs: server_defaults.header_read_timeout_secs,
                    request_body_read_timeout_secs: server_defaults.request_body_read_timeout_secs,
                    response_write_timeout_secs: server_defaults.response_write_timeout_secs,
                    access_log_format: server_defaults.access_log_format.clone(),
                    tls: None,
                },
                base_dir,
            )?;
            let http3 =
                compile_http3(binding.http3, compiled.server.listen_addr, binding.ssl, base_dir)?;

            Ok(Listener {
                id: vhost_listener_id(listen_addr),
                name: vhost_listener_name(listen_addr),
                server: compiled.server,
                tls_termination_enabled: binding.ssl,
                proxy_protocol_enabled: binding.proxy_protocol,
                http3,
            })
        })
        .collect()
}

fn explicit_listener_id(name: &str) -> String {
    format!("listener:{}", name.trim().to_ascii_lowercase())
}

struct VhostListenerBinding {
    ssl: bool,
    http3: Option<Http3Config>,
    proxy_protocol: bool,
}

fn vhost_listener_id(listen_addr: SocketAddr) -> String {
    format!("vhost-listen:{listen_addr}")
}

fn vhost_listener_name(listen_addr: SocketAddr) -> String {
    format!("vhost:{listen_addr}")
}
