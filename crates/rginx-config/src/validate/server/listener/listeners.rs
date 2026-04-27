use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;

use rginx_core::{Error, Result};

use crate::model::{Http3Config, ListenerConfig, ServerConfig, VirtualHostConfig};

use super::base::{ListenerLikeRef, legacy_server_listener_fields, validate_listener_like};

pub(super) fn validate_listeners(
    listeners: &[ListenerConfig],
    server: &ServerConfig,
    vhosts: &[VirtualHostConfig],
) -> Result<()> {
    let any_vhost_listen = vhosts.iter().any(|vhost| !vhost.listen.is_empty());

    if any_vhost_listen {
        if !listeners.is_empty() {
            return Err(Error::Config(
                "servers[].listen cannot be used together with top-level listeners".to_string(),
            ));
        }

        validate_server_fields_for_vhost_listen(server)?;
        validate_vhost_listener_bindings(vhosts)?;
        return Ok(());
    }

    if listeners.is_empty() {
        if server.listen.as_deref().is_none_or(str::is_empty) {
            return Err(Error::Config(
                "at least one listen must be configured in server.listen, listeners, or servers[].listen"
                    .to_string(),
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
    let mut all_listener_bindings = HashSet::new();
    for (index, listener) in listeners.iter().enumerate() {
        let owner = format!("listeners[{index}]");
        let normalized_name = listener.name.trim().to_ascii_lowercase();
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
            server_header: listener.server_header.as_deref(),
            proxy_protocol: listener.proxy_protocol,
            default_certificate: listener.default_certificate.as_deref(),
            trusted_proxies: &listener.trusted_proxies,
            client_ip_header: listener.client_ip_header.as_deref(),
            max_headers: listener.max_headers,
            max_request_body_bytes: listener.max_request_body_bytes,
            max_connections: listener.max_connections,
            header_read_timeout_secs: listener.header_read_timeout_secs,
            request_body_read_timeout_secs: listener.request_body_read_timeout_secs,
            response_write_timeout_secs: listener.response_write_timeout_secs,
            access_log_format: listener.access_log_format.as_deref(),
            tls: listener.tls.as_ref(),
            http3: listener.http3.as_ref(),
            require_listen: true,
        })?;

        let tcp_listen_addr = listener.listen.parse::<std::net::SocketAddr>().map_err(|error| {
            Error::Config(format!("{owner} listen `{}` is invalid: {error}", listener.listen))
        })?;
        if !all_listener_bindings.insert((rginx_core::ListenerTransportKind::Tcp, tcp_listen_addr))
        {
            return Err(Error::Config(format!(
                "duplicate tcp listener bind `{tcp_listen_addr}` across listeners"
            )));
        }

        if let Some(http3) = listener.http3.as_ref() {
            let udp_listen_addr = match http3.listen.as_deref() {
                Some(listen) => listen.parse::<std::net::SocketAddr>().map_err(|error| {
                    Error::Config(format!("{owner} http3 listen `{listen}` is invalid: {error}"))
                })?,
                None => tcp_listen_addr,
            };
            if !all_listener_bindings
                .insert((rginx_core::ListenerTransportKind::Udp, udp_listen_addr))
            {
                return Err(Error::Config(format!(
                    "duplicate udp listener bind `{udp_listen_addr}` across listeners"
                )));
            }
        }
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

fn validate_server_fields_for_vhost_listen(server: &ServerConfig) -> Result<()> {
    if server.listen.is_some() || server.proxy_protocol.is_some() || server.http3.is_some() {
        return Err(Error::Config(
            "server listen, proxy_protocol, and http3 cannot be used together with servers[].listen; use servers[].listen for bindings and keep server.tls/default_certificate only as global TLS defaults"
                .to_string(),
        ));
    }

    Ok(())
}

#[derive(Clone)]
struct VhostListenerBinding {
    ssl: bool,
    proxy_protocol: bool,
    http3: Option<Http3Config>,
}

fn validate_vhost_listener_bindings(vhosts: &[VirtualHostConfig]) -> Result<()> {
    let mut tcp_bindings = HashMap::<SocketAddr, VhostListenerBinding>::new();

    for (vhost_index, vhost) in vhosts.iter().enumerate() {
        for (listen_index, listen) in vhost.listen.iter().enumerate() {
            let owner = format!("servers[{vhost_index}].listen[{listen_index}]");
            let parsed = crate::listen::parse_vhost_listen(&owner, listen)?;
            let http3 = parsed.http3.then(|| vhost.http3.clone().unwrap_or_default());

            if let Some(existing) = tcp_bindings.get(&parsed.addr) {
                if existing.ssl != parsed.ssl {
                    return Err(Error::Config(format!(
                        "servers[].listen `{}` mixes ssl and non-ssl bindings",
                        parsed.addr
                    )));
                }
                if existing.proxy_protocol != parsed.proxy_protocol {
                    return Err(Error::Config(format!(
                        "servers[].listen `{}` mixes proxy_protocol and non-proxy_protocol bindings",
                        parsed.addr
                    )));
                }
                if existing.http3 != http3 {
                    return Err(Error::Config(format!(
                        "servers[].listen `{}` must use consistent http3 settings across vhosts",
                        parsed.addr
                    )));
                }
            } else {
                tcp_bindings.insert(
                    parsed.addr,
                    VhostListenerBinding {
                        ssl: parsed.ssl,
                        proxy_protocol: parsed.proxy_protocol,
                        http3,
                    },
                );
            }
        }
    }

    Ok(())
}
