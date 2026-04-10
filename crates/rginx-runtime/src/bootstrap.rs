use std::collections::{HashMap, HashSet};
use std::net::TcpListener as StdTcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rginx_core::{ConfigSnapshot, Error, Listener, Result};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::admin;
use crate::health;
use crate::ocsp;
use crate::reload;
use crate::restart::{self, ListenerHandle};
use crate::shutdown;
use crate::shutdown::RuntimeSignal;
use crate::state::RuntimeState;

pub async fn run(config_path: PathBuf, config: ConfigSnapshot) -> Result<()> {
    let state = RuntimeState::new(config_path, config)?;
    let current_config = state.current_config().await;
    let inherited_listeners = restart::take_inherited_listeners_from_env()?;
    let mut active_listener_groups = build_initial_listener_groups(
        &current_config.listeners,
        current_config.runtime.accept_workers,
        inherited_listeners,
        state.http.clone(),
    )
    .await?;
    let mut draining_listener_groups = Vec::new();
    let (shutdown_tx, _shutdown_rx) = watch::channel(false);

    tracing::info!(
        listeners = current_config.total_listener_count(),
        worker_threads = current_config.runtime.worker_threads,
        accept_workers = current_config.runtime.accept_workers,
        vhosts = current_config.total_vhost_count(),
        routes = current_config.total_route_count(),
        "starting rginx runtime"
    );

    let mut admin_task = tokio::spawn(admin::run(
        state.config_path.clone(),
        state.http.clone(),
        shutdown_tx.subscribe(),
    ));
    let mut health_task = tokio::spawn(health::run(state.http.clone(), shutdown_tx.subscribe()));
    let mut ocsp_task = tokio::spawn(ocsp::run(state.http.clone(), shutdown_tx.subscribe()));
    restart::notify_ready_if_requested()?;

    loop {
        prune_draining_listener_groups(&state.http, &mut draining_listener_groups).await;
        let signal = if draining_listener_groups.is_empty() {
            shutdown::wait_for_signal().await?
        } else {
            tokio::select! {
                signal = shutdown::wait_for_signal() => signal?,
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    continue;
                }
            }
        };

        match signal {
            RuntimeSignal::Reload => {
                tracing::info!("reload signal received");
                match reload::prepare_reload(&state).await {
                    Ok(pending) => {
                        match prepare_added_listener_bindings(
                            &pending.next_config.listeners,
                            pending.next_config.runtime.accept_workers,
                            &active_listener_groups,
                            &draining_listener_groups,
                        ) {
                            Ok(prepared_additions) => {
                                match reload::commit_reload(&state, pending).await {
                                    Ok(result) => {
                                        reconcile_listener_worker_groups(
                                            &state.http,
                                            &result.config,
                                            prepared_additions,
                                            &mut active_listener_groups,
                                            &mut draining_listener_groups,
                                        )
                                        .await;
                                        tracing::info!(
                                            revision = result.revision,
                                            listeners = result.config.total_listener_count(),
                                            vhosts = result.config.total_vhost_count(),
                                            routes = result.config.total_route_count(),
                                            upstreams = result.config.upstreams.len(),
                                            "configuration reloaded"
                                        );
                                    }
                                    Err(error) => {
                                        tracing::warn!(%error, "configuration reload failed");
                                    }
                                }
                            }
                            Err(error) => {
                                state.http.record_reload_failure(
                                    error.to_string(),
                                    pending.current_revision,
                                );
                                tracing::warn!(%error, "configuration reload failed");
                            }
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "configuration reload failed");
                    }
                }
            }
            RuntimeSignal::Restart => {
                tracing::info!("restart signal received");
                let handles = active_listener_groups
                    .values()
                    .map(ListenerWorkerGroup::restart_handle)
                    .collect::<Vec<_>>();
                match restart::restart(&state.config_path, &handles).await {
                    Ok(()) => {
                        tracing::info!(
                            "replacement process became ready; starting graceful handoff"
                        );
                        break;
                    }
                    Err(error) => {
                        tracing::warn!(%error, "graceful restart failed");
                    }
                }
            }
            RuntimeSignal::Shutdown => break,
        }
    }

    let current_config = state.current_config().await;

    tracing::info!(
        timeout_secs = current_config.runtime.shutdown_timeout.as_secs(),
        "graceful shutdown requested"
    );
    let _ = shutdown_tx.send(true);
    initiate_shutdown_for_groups(active_listener_groups.values());
    initiate_shutdown_for_groups(draining_listener_groups.iter());

    match tokio::time::timeout(current_config.runtime.shutdown_timeout, async {
        join_listener_worker_groups(
            &state.http,
            &mut active_listener_groups,
            &mut draining_listener_groups,
        )
        .await?;
        (&mut admin_task).await.map_err(|error| {
            Error::Server(format!("admin socket task failed to join: {error}"))
        })??;
        (&mut health_task).await.map_err(|error| {
            Error::Server(format!("active health task failed to join: {error}"))
        })?;
        (&mut ocsp_task)
            .await
            .map_err(|error| Error::Server(format!("OCSP refresh task failed to join: {error}")))?;
        state.http.drain_background_tasks().await;
        Ok::<(), Error>(())
    })
    .await
    {
        Ok(join_result) => {
            join_result?;
        }
        Err(_) => {
            tracing::warn!(
                "shutdown timeout reached before background tasks drained all active work"
            );
            abort_listener_worker_groups(active_listener_groups.values());
            abort_listener_worker_groups(draining_listener_groups.iter());
            admin_task.abort();
            health_task.abort();
            ocsp_task.abort();

            join_aborted_listener_worker_groups(
                &state.http,
                &mut active_listener_groups,
                &mut draining_listener_groups,
            )
            .await;

            if let Err(error) = admin_task.await
                && !error.is_cancelled()
            {
                tracing::warn!(%error, "admin socket task failed after abort");
            }

            if let Err(error) = health_task.await
                && !error.is_cancelled()
            {
                tracing::warn!(%error, "active health task failed after abort");
            }

            if let Err(error) = ocsp_task.await
                && !error.is_cancelled()
            {
                tracing::warn!(%error, "OCSP refresh task failed after abort");
            }

            state.http.abort_background_tasks().await;
        }
    }

    Ok(())
}

struct PreparedListenerWorkerGroup {
    listener: Listener,
    std_listener: Arc<StdTcpListener>,
    worker_listeners: Vec<TcpListener>,
}

struct ListenerWorkerGroup {
    listener: Listener,
    std_listener: Arc<StdTcpListener>,
    shutdown_tx: watch::Sender<bool>,
    tasks: Vec<JoinHandle<Result<()>>>,
}

impl ListenerWorkerGroup {
    fn restart_handle(&self) -> ListenerHandle {
        ListenerHandle { listener: self.listener.clone(), std_listener: self.std_listener.clone() }
    }

    fn initiate_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    fn abort(&self) {
        for task in &self.tasks {
            task.abort();
        }
    }

    fn is_finished(&self) -> bool {
        self.tasks.iter().all(JoinHandle::is_finished)
    }
}

async fn build_initial_listener_groups(
    listeners: &[Listener],
    accept_workers: usize,
    mut inherited: HashMap<std::net::SocketAddr, StdTcpListener>,
    http_state: rginx_http::SharedState,
) -> Result<HashMap<String, ListenerWorkerGroup>> {
    let mut groups = HashMap::new();

    for listener in listeners {
        let std_listener = match inherited.remove(&listener.server.listen_addr) {
            Some(listener_socket) => listener_socket,
            None => bind_std_listener(listener.server.listen_addr)?,
        };
        let prepared = prepare_listener_worker_group(
            listener.clone(),
            Arc::new(std_listener),
            accept_workers,
        )?;
        let group = activate_prepared_listener_worker_group(prepared, http_state.clone());
        groups.insert(listener.id.clone(), group);
    }

    Ok(groups)
}

fn prepare_added_listener_bindings(
    next_listeners: &[Listener],
    accept_workers: usize,
    active_listener_groups: &HashMap<String, ListenerWorkerGroup>,
    draining_listener_groups: &[ListenerWorkerGroup],
) -> Result<Vec<PreparedListenerWorkerGroup>> {
    let active_ids = active_listener_groups.keys().cloned().collect::<HashSet<_>>();
    let draining_ids = draining_listener_groups
        .iter()
        .map(|group| group.listener.id.clone())
        .collect::<HashSet<_>>();
    let active_addrs = active_listener_groups
        .values()
        .map(|group| group.listener.server.listen_addr)
        .collect::<HashSet<_>>();
    let draining_addrs = draining_listener_groups
        .iter()
        .map(|group| group.listener.server.listen_addr)
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
        if active_addrs.contains(&listener.server.listen_addr)
            || draining_addrs.contains(&listener.server.listen_addr)
        {
            return Err(Error::Server(format!(
                "listener `{}` reuses listen address `{}` with a different listener identity during reload",
                listener.name, listener.server.listen_addr
            )));
        }
        prepared.push(prepare_listener_worker_group(
            listener.clone(),
            Arc::new(bind_std_listener(listener.server.listen_addr)?),
            accept_workers,
        )?);
    }

    Ok(prepared)
}

async fn reconcile_listener_worker_groups(
    http_state: &rginx_http::SharedState,
    next_config: &ConfigSnapshot,
    prepared_additions: Vec<PreparedListenerWorkerGroup>,
    active_listener_groups: &mut HashMap<String, ListenerWorkerGroup>,
    draining_listener_groups: &mut Vec<ListenerWorkerGroup>,
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
        let group = activate_prepared_listener_worker_group(prepared, http_state.clone());
        active_listener_groups.insert(listener_id, group);
    }

    prune_draining_listener_groups(http_state, draining_listener_groups).await;
}

fn prepare_listener_worker_group(
    listener: Listener,
    std_listener: Arc<StdTcpListener>,
    accept_workers: usize,
) -> Result<PreparedListenerWorkerGroup> {
    let mut worker_listeners = Vec::new();
    for _worker_index in 0..accept_workers {
        worker_listeners.push(TcpListener::from_std(std_listener.try_clone()?)?);
    }

    Ok(PreparedListenerWorkerGroup { listener, std_listener, worker_listeners })
}

fn activate_prepared_listener_worker_group(
    prepared: PreparedListenerWorkerGroup,
    http_state: rginx_http::SharedState,
) -> ListenerWorkerGroup {
    let (shutdown_tx, _shutdown_rx) = watch::channel(false);
    let mut tasks = Vec::new();

    for (worker_index, listener_socket) in prepared.worker_listeners.into_iter().enumerate() {
        tracing::info!(
            listener = %prepared.listener.name,
            listen = %prepared.listener.server.listen_addr,
            worker = worker_index,
            "starting accept worker"
        );
        tasks.push(tokio::spawn(rginx_http::serve(
            listener_socket,
            prepared.listener.id.clone(),
            http_state.clone(),
            shutdown_tx.subscribe(),
        )));
    }

    ListenerWorkerGroup {
        listener: prepared.listener,
        std_listener: prepared.std_listener,
        shutdown_tx,
        tasks,
    }
}

async fn prune_draining_listener_groups(
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

async fn join_listener_worker_groups(
    http_state: &rginx_http::SharedState,
    active_listener_groups: &mut HashMap<String, ListenerWorkerGroup>,
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

fn initiate_shutdown_for_groups<'a>(groups: impl IntoIterator<Item = &'a ListenerWorkerGroup>) {
    for group in groups {
        group.initiate_shutdown();
    }
}

fn abort_listener_worker_groups<'a>(groups: impl IntoIterator<Item = &'a ListenerWorkerGroup>) {
    for group in groups {
        group.abort();
    }
}

async fn join_aborted_listener_worker_groups(
    http_state: &rginx_http::SharedState,
    active_listener_groups: &mut HashMap<String, ListenerWorkerGroup>,
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
