use std::collections::{HashMap, HashSet};
use std::net::{TcpListener as StdTcpListener, UdpSocket as StdUdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use rginx_core::{ConfigSnapshot, Error, Listener, Result, VirtualHost};
use tokio::net::TcpListener;
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

use crate::restart::{InheritedListeners, ListenerHandle};

pub(super) type ListenerGroupMap = HashMap<String, ListenerWorkerGroup>;

pub(super) struct PreparedListenerWorkerGroup {
    listener: Listener,
    std_listener: Arc<StdTcpListener>,
    std_udp_socket: Option<Arc<StdUdpSocket>>,
    worker_listeners: Vec<TcpListener>,
    http3_endpoint: Option<quinn::Endpoint>,
}

pub(super) struct WorkerDrainGuard {
    remaining_workers: Arc<AtomicUsize>,
    drain_completion_notify: Arc<Notify>,
}

impl Drop for WorkerDrainGuard {
    fn drop(&mut self) {
        if self.remaining_workers.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.drain_completion_notify.notify_waiters();
        }
    }
}

pub(super) struct ListenerWorkerGroup {
    pub(super) listener: Listener,
    std_listener: Arc<StdTcpListener>,
    std_udp_socket: Option<Arc<StdUdpSocket>>,
    shutdown_tx: watch::Sender<bool>,
    tasks: Vec<JoinHandle<Result<()>>>,
}

impl ListenerWorkerGroup {
    pub(super) fn restart_handle(&self) -> ListenerHandle {
        ListenerHandle {
            listener: self.listener.clone(),
            std_listener: self.std_listener.clone(),
            std_udp_socket: self.std_udp_socket.clone(),
        }
    }

    pub(super) fn initiate_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    pub(super) fn abort(&self) {
        for task in &self.tasks {
            task.abort();
        }
    }

    fn is_finished(&self) -> bool {
        self.tasks.iter().all(JoinHandle::is_finished)
    }
}

pub(super) async fn build_initial_listener_groups(
    config: &ConfigSnapshot,
    mut inherited: InheritedListeners,
    http_state: rginx_http::SharedState,
    drain_completion_notify: Arc<Notify>,
) -> Result<ListenerGroupMap> {
    let mut groups = HashMap::new();

    for listener in &config.listeners {
        let std_listener = match inherited.tcp.remove(&listener.server.listen_addr) {
            Some(listener_socket) => listener_socket,
            None => bind_std_listener(listener.server.listen_addr)?,
        };
        let std_udp_socket = match &listener.http3 {
            Some(http3) => match inherited.udp.remove(&http3.listen_addr) {
                Some(socket) => Some(Arc::new(socket)),
                None => Some(Arc::new(bind_std_udp_socket(http3.listen_addr)?)),
            },
            None => None,
        };
        let prepared = prepare_listener_worker_group(
            listener.clone(),
            Arc::new(std_listener),
            std_udp_socket,
            config.runtime.accept_workers,
            &config.default_vhost,
            &config.vhosts,
        )?;
        let group = activate_prepared_listener_worker_group(
            prepared,
            http_state.clone(),
            drain_completion_notify.clone(),
        );
        groups.insert(listener.id.clone(), group);
    }

    Ok(groups)
}

pub(super) fn prepare_added_listener_bindings(
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
                .map(|http3| bind_std_udp_socket(http3.listen_addr))
                .transpose()?
                .map(Arc::new),
            accept_workers,
            &next_config.default_vhost,
            &next_config.vhosts,
        )?);
    }

    Ok(prepared)
}

pub(super) async fn reconcile_listener_worker_groups(
    http_state: &rginx_http::SharedState,
    next_config: &ConfigSnapshot,
    prepared_additions: Vec<PreparedListenerWorkerGroup>,
    active_listener_groups: &mut ListenerGroupMap,
    draining_listener_groups: &mut Vec<ListenerWorkerGroup>,
    drain_completion_notify: Arc<Notify>,
) {
    let next_by_id = next_config
        .listeners
        .iter()
        .map(|listener| (listener.id.clone(), listener.clone()))
        .collect::<HashMap<_, _>>();

    let removed_ids = active_listener_groups
        .keys()
        .filter(|listener_id| !next_by_id.contains_key(*listener_id))
        .cloned()
        .collect::<Vec<_>>();

    for removed_id in removed_ids {
        if let Some(group) = active_listener_groups.remove(&removed_id) {
            http_state.retire_listener_runtime(&group.listener);
            group.initiate_shutdown();
            tracing::info!(
                listener = %group.listener.name,
                listen = %group.listener.server.listen_addr,
                "listener removed from active config; draining existing connections"
            );
            draining_listener_groups.push(group);
        }
    }

    for (listener_id, group) in active_listener_groups.iter_mut() {
        let next_listener = next_by_id
            .get(listener_id)
            .expect("active listener ids should remain present in the next config");
        group.listener = next_listener.clone();
    }

    for prepared in prepared_additions {
        let listener_id = prepared.listener.id.clone();
        let group = activate_prepared_listener_worker_group(
            prepared,
            http_state.clone(),
            drain_completion_notify.clone(),
        );
        active_listener_groups.insert(listener_id, group);
    }

    prune_draining_listener_groups(http_state, draining_listener_groups).await;
}

pub(super) async fn prune_draining_listener_groups(
    http_state: &rginx_http::SharedState,
    draining_listener_groups: &mut Vec<ListenerWorkerGroup>,
) {
    let mut index = 0usize;
    while index < draining_listener_groups.len() {
        if !draining_listener_groups[index].is_finished() {
            index += 1;
            continue;
        }

        let mut group = draining_listener_groups.remove(index);
        let listener_id = group.listener.id.clone();
        if let Err(error) = join_listener_worker_group(&mut group).await {
            tracing::warn!(%error, listener_id = %listener_id, "listener drain failed");
        }
        http_state.remove_retired_listener_runtime(&listener_id).await;
    }
}

pub(super) async fn join_listener_worker_groups(
    http_state: &rginx_http::SharedState,
    active_listener_groups: &mut ListenerGroupMap,
    draining_listener_groups: &mut Vec<ListenerWorkerGroup>,
) -> Result<()> {
    for group in active_listener_groups.values_mut() {
        join_listener_worker_group(group).await?;
    }
    active_listener_groups.clear();

    for group in draining_listener_groups.iter_mut() {
        let listener_id = group.listener.id.clone();
        join_listener_worker_group(group).await?;
        http_state.remove_retired_listener_runtime(&listener_id).await;
    }
    draining_listener_groups.clear();

    Ok(())
}

pub(super) fn initiate_shutdown_for_groups<'a>(
    groups: impl IntoIterator<Item = &'a ListenerWorkerGroup>,
) {
    for group in groups {
        group.initiate_shutdown();
    }
}

pub(super) fn abort_listener_worker_groups<'a>(
    groups: impl IntoIterator<Item = &'a ListenerWorkerGroup>,
) {
    for group in groups {
        group.abort();
    }
}

pub(super) async fn join_aborted_listener_worker_groups(
    http_state: &rginx_http::SharedState,
    active_listener_groups: &mut ListenerGroupMap,
    draining_listener_groups: &mut Vec<ListenerWorkerGroup>,
) {
    for group in active_listener_groups.values_mut() {
        join_aborted_listener_worker_group(group).await;
    }
    active_listener_groups.clear();

    for group in draining_listener_groups.iter_mut() {
        let listener_id = group.listener.id.clone();
        join_aborted_listener_worker_group(group).await;
        http_state.remove_retired_listener_runtime(&listener_id).await;
    }
    draining_listener_groups.clear();
}

fn prepare_listener_worker_group(
    listener: Listener,
    std_listener: Arc<StdTcpListener>,
    std_udp_socket: Option<Arc<StdUdpSocket>>,
    accept_workers: usize,
    default_vhost: &VirtualHost,
    vhosts: &[VirtualHost],
) -> Result<PreparedListenerWorkerGroup> {
    let mut worker_listeners = Vec::new();
    for _worker_index in 0..accept_workers {
        worker_listeners.push(TcpListener::from_std(std_listener.try_clone()?)?);
    }
    let http3_endpoint = match &std_udp_socket {
        Some(socket) => Some(rginx_http::server::bind_http3_endpoint_with_socket(
            &listener,
            default_vhost,
            vhosts,
            socket.try_clone()?,
        )?),
        None => rginx_http::server::bind_http3_endpoint(&listener, default_vhost, vhosts)?,
    };

    Ok(PreparedListenerWorkerGroup {
        listener,
        std_listener,
        std_udp_socket,
        worker_listeners,
        http3_endpoint,
    })
}

fn activate_prepared_listener_worker_group(
    prepared: PreparedListenerWorkerGroup,
    http_state: rginx_http::SharedState,
    drain_completion_notify: Arc<Notify>,
) -> ListenerWorkerGroup {
    let (shutdown_tx, _shutdown_rx) = watch::channel(false);
    let mut tasks = Vec::new();
    let remaining_workers = Arc::new(AtomicUsize::new(
        prepared.worker_listeners.len() + usize::from(prepared.http3_endpoint.is_some()),
    ));

    for (worker_index, listener_socket) in prepared.worker_listeners.into_iter().enumerate() {
        tracing::info!(
            listener = %prepared.listener.name,
            listen = %prepared.listener.server.listen_addr,
            worker = worker_index,
            "starting accept worker"
        );
        let listener_id = prepared.listener.id.clone();
        let http_state = http_state.clone();
        let shutdown = shutdown_tx.subscribe();
        let remaining_workers = remaining_workers.clone();
        let drain_completion_notify = drain_completion_notify.clone();
        tasks.push(tokio::spawn(async move {
            let _drain_guard = WorkerDrainGuard { remaining_workers, drain_completion_notify };
            rginx_http::serve(listener_socket, listener_id, http_state, shutdown).await
        }));
    }

    if let Some(endpoint) = prepared.http3_endpoint {
        tracing::info!(
            listener = %prepared.listener.name,
            listen = %prepared
                .listener
                .http3
                .as_ref()
                .map(|http3| http3.listen_addr)
                .unwrap_or(prepared.listener.server.listen_addr),
            "starting http3 accept worker"
        );
        let listener_id = prepared.listener.id.clone();
        let http_state = http_state.clone();
        let shutdown = shutdown_tx.subscribe();
        let remaining_workers = remaining_workers.clone();
        let drain_completion_notify = drain_completion_notify.clone();
        tasks.push(tokio::spawn(async move {
            let _drain_guard = WorkerDrainGuard { remaining_workers, drain_completion_notify };
            rginx_http::server::serve_http3(endpoint, listener_id, http_state, shutdown).await
        }));
    }

    ListenerWorkerGroup {
        listener: prepared.listener,
        std_listener: prepared.std_listener,
        std_udp_socket: prepared.std_udp_socket,
        shutdown_tx,
        tasks,
    }
}

async fn join_listener_worker_group(group: &mut ListenerWorkerGroup) -> Result<()> {
    for (worker_index, task) in group.tasks.iter_mut().enumerate() {
        task.await.map_err(|error| {
            Error::Server(format!(
                "listener `{}` worker {worker_index} failed to join: {error}",
                group.listener.name
            ))
        })??;
    }
    group.tasks.clear();
    Ok(())
}

async fn join_aborted_listener_worker_group(group: &mut ListenerWorkerGroup) {
    for task in group.tasks.iter_mut() {
        if let Err(error) = task.await
            && !error.is_cancelled()
        {
            tracing::warn!(%error, listener = %group.listener.name, "http worker failed after abort");
        }
    }
    group.tasks.clear();
}

fn bind_std_listener(listen_addr: std::net::SocketAddr) -> Result<StdTcpListener> {
    let socket = StdTcpListener::bind(listen_addr)?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}

fn bind_std_udp_socket(listen_addr: std::net::SocketAddr) -> Result<StdUdpSocket> {
    let socket = StdUdpSocket::bind(listen_addr)?;
    socket.set_nonblocking(true)?;
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::net::SocketAddr;

    use rginx_core::Server;

    use super::*;

    fn listener(id: &str, name: &str, listen_addr: SocketAddr) -> Listener {
        Listener {
            id: id.to_string(),
            name: name.to_string(),
            server: Server {
                listen_addr,
                default_certificate: None,
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                request_body_read_timeout: None,
                response_write_timeout: None,
                access_log_format: None,
                tls: None,
            },
            tls_termination_enabled: false,
            proxy_protocol_enabled: false,
            http3: None,
        }
    }

    fn config_with_listeners(listeners: Vec<Listener>) -> ConfigSnapshot {
        ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings {
                shutdown_timeout: std::time::Duration::from_secs(1),
                worker_threads: None,
                accept_workers: 1,
            },
            listeners,
            default_vhost: rginx_core::VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            upstreams: HashMap::new(),
        }
    }

    fn listener_group_with_socket(
        listener: Listener,
        std_listener: StdTcpListener,
    ) -> ListenerWorkerGroup {
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        ListenerWorkerGroup {
            listener,
            std_listener: Arc::new(std_listener),
            std_udp_socket: None,
            shutdown_tx,
            tasks: Vec::new(),
        }
    }

    #[test]
    fn prepare_added_listener_bindings_rejects_active_addr_reuse_with_new_id() {
        let std_listener =
            bind_std_listener("127.0.0.1:0".parse().expect("socket addr should parse")).unwrap();
        let listen_addr = std_listener.local_addr().expect("listener addr should exist");

        let active_listener = listener("listener-a", "listener-a", listen_addr);
        let active_groups = HashMap::from([(
            active_listener.id.clone(),
            listener_group_with_socket(active_listener, std_listener),
        )]);
        let next_config =
            config_with_listeners(vec![listener("listener-b", "listener-b", listen_addr)]);
        let error = match prepare_added_listener_bindings(
            &next_config,
            &[listener("listener-b", "listener-b", listen_addr)],
            1,
            &active_groups,
            &[],
        ) {
            Ok(_) => panic!("reusing an active listen addr with a new id must fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("reuses tcp listen address"));
    }

    #[test]
    fn prepare_added_listener_bindings_rejects_draining_addr_reuse_with_new_id() {
        let std_listener =
            bind_std_listener("127.0.0.1:0".parse().expect("socket addr should parse")).unwrap();
        let listen_addr = std_listener.local_addr().expect("listener addr should exist");

        let draining_groups = vec![listener_group_with_socket(
            listener("listener-a", "listener-a", listen_addr),
            std_listener,
        )];
        let next_config =
            config_with_listeners(vec![listener("listener-b", "listener-b", listen_addr)]);
        let error = match prepare_added_listener_bindings(
            &next_config,
            &[listener("listener-b", "listener-b", listen_addr)],
            1,
            &HashMap::new(),
            &draining_groups,
        ) {
            Ok(_) => panic!("reusing a draining listen addr with a new id must fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("reuses tcp listen address"));
    }
}
