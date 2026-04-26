use std::collections::{HashMap, HashSet};
use std::net::{TcpListener as StdTcpListener, UdpSocket as StdUdpSocket};
use std::sync::Arc;

use rginx_core::{ConfigSnapshot, Error, Listener, Result, VirtualHost};
use tokio::net::TcpListener;

use crate::restart::InheritedListeners;

use super::bind_tcp::bind_std_listener;
use super::bind_udp::{bind_std_udp_sockets, normalize_inherited_udp_sockets};
use super::{ListenerGroupMap, ListenerWorkerGroup};

pub(crate) struct PreparedListenerWorkerGroup {
    pub(super) listener: Listener,
    pub(super) std_listener: Arc<StdTcpListener>,
    pub(super) std_udp_sockets: Vec<Arc<StdUdpSocket>>,
    pub(super) worker_listeners: Vec<TcpListener>,
    pub(super) http3_endpoints: Vec<quinn::Endpoint>,
}

pub(crate) async fn build_initial_listener_groups(
    config: &ConfigSnapshot,
    mut inherited: InheritedListeners,
    http_state: rginx_http::SharedState,
    drain_completion_notify: Arc<tokio::sync::Notify>,
) -> Result<ListenerGroupMap> {
    let mut groups = HashMap::new();

    for listener in &config.listeners {
        let std_listener = match inherited.tcp.remove(&listener.server.listen_addr) {
            Some(listener_socket) => listener_socket,
            None => bind_std_listener(listener.server.listen_addr)?,
        };
        let desired_udp_socket_count = config.runtime.accept_workers.max(1);
        let std_udp_sockets = match &listener.http3 {
            Some(http3) => match inherited.udp.remove(&http3.listen_addr) {
                Some(sockets) => normalize_inherited_udp_sockets(
                    &listener.name,
                    http3.listen_addr,
                    sockets,
                    desired_udp_socket_count,
                )?,
                None => bind_std_udp_sockets(http3.listen_addr, config.runtime.accept_workers)?,
            },
            None => Vec::new(),
        };
        let prepared = prepare_listener_worker_group(
            listener.clone(),
            Arc::new(std_listener),
            std_udp_sockets,
            config.runtime.accept_workers,
            &config.default_vhost,
            &config.vhosts,
        )?;
        let group = super::activate::activate_prepared_listener_worker_group(
            prepared,
            http_state.clone(),
            drain_completion_notify.clone(),
        );
        groups.insert(listener.id.clone(), group);
    }

    Ok(groups)
}

pub(crate) fn prepare_added_listener_bindings(
    next_config: &ConfigSnapshot,
    next_listeners: &[Listener],
    accept_workers: usize,
    active_listener_groups: &ListenerGroupMap,
    draining_listener_groups: &[ListenerWorkerGroup],
) -> Result<Vec<PreparedListenerWorkerGroup>> {
    let active_ids = active_listener_groups.keys().cloned().collect::<HashSet<_>>();
    let draining_ids = draining_listener_groups
        .iter()
        .map(|group| group.listener.id.clone())
        .collect::<HashSet<_>>();
    let active_addrs = active_listener_groups
        .values()
        .flat_map(|group| {
            group
                .listener
                .transport_bindings()
                .into_iter()
                .map(|binding| (binding.kind, binding.listen_addr))
        })
        .collect::<HashSet<_>>();
    let draining_addrs = draining_listener_groups
        .iter()
        .flat_map(|group| {
            group
                .listener
                .transport_bindings()
                .into_iter()
                .map(|binding| (binding.kind, binding.listen_addr))
        })
        .collect::<HashSet<_>>();

    let mut prepared = Vec::new();
    for listener in next_listeners {
        if active_ids.contains(&listener.id) {
            continue;
        }
        if draining_ids.contains(&listener.id) {
            return Err(Error::Server(format!(
                "listener `{}` cannot be re-added until the previous generation has drained",
                listener.name
            )));
        }
        for binding in listener.transport_bindings() {
            let key = (binding.kind, binding.listen_addr);
            if active_addrs.contains(&key) || draining_addrs.contains(&key) {
                return Err(Error::Server(format!(
                    "listener `{}` reuses {} listen address `{}` with a different listener identity during reload",
                    listener.name,
                    binding.kind.as_str(),
                    binding.listen_addr
                )));
            }
        }
        prepared.push(prepare_listener_worker_group(
            listener.clone(),
            Arc::new(bind_std_listener(listener.server.listen_addr)?),
            listener
                .http3
                .as_ref()
                .map(|http3| bind_std_udp_sockets(http3.listen_addr, accept_workers))
                .transpose()?
                .unwrap_or_default(),
            accept_workers,
            &next_config.default_vhost,
            &next_config.vhosts,
        )?);
    }

    Ok(prepared)
}

fn prepare_listener_worker_group(
    listener: Listener,
    std_listener: Arc<StdTcpListener>,
    std_udp_sockets: Vec<Arc<StdUdpSocket>>,
    accept_workers: usize,
    default_vhost: &VirtualHost,
    vhosts: &[VirtualHost],
) -> Result<PreparedListenerWorkerGroup> {
    let mut worker_listeners = Vec::new();
    for _worker_index in 0..accept_workers {
        worker_listeners.push(TcpListener::from_std(std_listener.try_clone()?)?);
    }
    let mut http3_endpoints = Vec::new();
    for socket in &std_udp_sockets {
        http3_endpoints.push(rginx_http::server::bind_http3_endpoint_with_socket(
            &listener,
            default_vhost,
            vhosts,
            socket.try_clone()?,
        )?);
    }

    Ok(PreparedListenerWorkerGroup {
        listener,
        std_listener,
        std_udp_sockets,
        worker_listeners,
        http3_endpoints,
    })
}
