use std::collections::HashSet;

use rginx_core::{Error, Result};

use crate::model::{ListenerConfig, ServerConfig, VirtualHostConfig};

use super::base::{ListenerLikeRef, legacy_server_listener_fields, validate_listener_like};

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
