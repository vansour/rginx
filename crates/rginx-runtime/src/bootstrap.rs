use std::path::PathBuf;

use rginx_core::{ConfigSnapshot, Error, Listener, Result};
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::admin;
use crate::health;
use crate::reload;
use crate::shutdown;
use crate::shutdown::RuntimeSignal;
use crate::state::RuntimeState;

pub async fn run(config_path: PathBuf, config: ConfigSnapshot) -> Result<()> {
    let state = RuntimeState::new(config_path, config)?;
    let current_config = state.current_config().await;
    let listeners =
        bind_server_listeners(&current_config.listeners, current_config.runtime.accept_workers)
            .await?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tracing::info!(
        listeners = current_config.total_listener_count(),
        worker_threads = current_config.runtime.worker_threads,
        accept_workers = current_config.runtime.accept_workers,
        vhosts = current_config.total_vhost_count(),
        routes = current_config.total_route_count(),
        "starting rginx runtime"
    );

    let mut server_tasks = listeners
        .into_iter()
        .map(|bound_listener| {
            tracing::info!(
                listener = %bound_listener.listener_name,
                listen = %bound_listener.listen_addr,
                worker = bound_listener.worker_index,
                "starting accept worker"
            );
            tokio::spawn(rginx_http::serve(
                bound_listener.listener,
                bound_listener.listener_id,
                state.http.clone(),
                shutdown_rx.clone(),
            ))
        })
        .collect::<Vec<_>>();
    let mut admin_task = tokio::spawn(admin::run(
        state.config_path.clone(),
        state.http.clone(),
        shutdown_tx.subscribe(),
    ));
    let mut health_task = tokio::spawn(health::run(state.http.clone(), shutdown_tx.subscribe()));

    while let RuntimeSignal::Reload = shutdown::wait_for_signal().await? {
        tracing::info!("reload signal received");
        match reload::reload(&state).await {
            Ok(result) => {
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

    let current_config = state.current_config().await;

    tracing::info!(
        timeout_secs = current_config.runtime.shutdown_timeout.as_secs(),
        "graceful shutdown requested"
    );
    let _ = shutdown_tx.send(true);

    match tokio::time::timeout(current_config.runtime.shutdown_timeout, async {
        join_server_tasks(&mut server_tasks).await?;
        (&mut admin_task).await.map_err(|error| {
            Error::Server(format!("admin socket task failed to join: {error}"))
        })??;
        (&mut health_task).await.map_err(|error| {
            Error::Server(format!("active health task failed to join: {error}"))
        })?;
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
            abort_server_tasks(&server_tasks);
            admin_task.abort();
            health_task.abort();

            join_aborted_server_tasks(server_tasks).await;

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

            state.http.abort_background_tasks().await;
        }
    }

    Ok(())
}

async fn bind_server_listeners(
    listeners: &[Listener],
    accept_workers: usize,
) -> Result<Vec<BoundListener>> {
    let mut bound_listeners = Vec::new();

    for listener in listeners {
        let socket = TcpListener::bind(listener.server.listen_addr).await?;
        let std_listener = socket.into_std()?;

        for worker_index in 0..accept_workers {
            let listener_socket = TcpListener::from_std(std_listener.try_clone()?)?;
            bound_listeners.push(BoundListener {
                listener: listener_socket,
                listener_id: listener.id.clone(),
                listener_name: listener.name.clone(),
                listen_addr: listener.server.listen_addr,
                worker_index,
            });
        }
    }

    Ok(bound_listeners)
}

struct BoundListener {
    listener: TcpListener,
    listener_id: String,
    listener_name: String,
    listen_addr: std::net::SocketAddr,
    worker_index: usize,
}

async fn join_server_tasks(server_tasks: &mut [tokio::task::JoinHandle<Result<()>>]) -> Result<()> {
    for (worker_index, server_task) in server_tasks.iter_mut().enumerate() {
        server_task.await.map_err(|error| {
            Error::Server(format!("http worker {worker_index} failed to join: {error}"))
        })??;
    }

    Ok(())
}

fn abort_server_tasks(server_tasks: &[tokio::task::JoinHandle<Result<()>>]) {
    for server_task in server_tasks {
        server_task.abort();
    }
}

async fn join_aborted_server_tasks(server_tasks: Vec<tokio::task::JoinHandle<Result<()>>>) {
    for server_task in server_tasks {
        if let Err(error) = server_task.await
            && !error.is_cancelled()
        {
            tracing::warn!(%error, "http worker failed after abort");
        }
    }
}
