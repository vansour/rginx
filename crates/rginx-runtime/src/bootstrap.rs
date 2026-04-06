use std::path::PathBuf;

use rginx_core::{ConfigSnapshot, Error, Result};
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::health;
use crate::reload;
use crate::shutdown;
use crate::shutdown::RuntimeSignal;
use crate::state::RuntimeState;

pub async fn run(config_path: PathBuf, config: ConfigSnapshot) -> Result<()> {
    let state = RuntimeState::new(config_path, config)?;
    let current_config = state.current_config().await;
    let listeners = bind_server_listeners(
        current_config.server.listen_addr,
        current_config.runtime.accept_workers,
    )
    .await?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tracing::info!(
        listen = %current_config.server.listen_addr,
        worker_threads = current_config.runtime.worker_threads,
        accept_workers = current_config.runtime.accept_workers,
        vhosts = current_config.total_vhost_count(),
        routes = current_config.total_route_count(),
        "starting rginx runtime"
    );

    let mut server_tasks = listeners
        .into_iter()
        .enumerate()
        .map(|(worker_index, listener)| {
            tracing::info!(worker = worker_index, "starting accept worker");
            tokio::spawn(rginx_http::serve(listener, state.http.clone(), shutdown_rx.clone()))
        })
        .collect::<Vec<_>>();
    let mut health_task = tokio::spawn(health::run(state.http.clone(), shutdown_tx.subscribe()));

    while let RuntimeSignal::Reload = shutdown::wait_for_signal().await? {
        tracing::info!("reload signal received");
        match reload::reload(&state).await {
            Ok(config) => {
                tracing::info!(
                    listen = %config.server.listen_addr,
                    vhosts = config.total_vhost_count(),
                    routes = config.total_route_count(),
                    upstreams = config.upstreams.len(),
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
            health_task.abort();

            join_aborted_server_tasks(server_tasks).await;

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
    listen_addr: std::net::SocketAddr,
    accept_workers: usize,
) -> Result<Vec<TcpListener>> {
    let listener = TcpListener::bind(listen_addr).await?;
    let std_listener = listener.into_std()?;
    let mut listeners = Vec::with_capacity(accept_workers);

    for _ in 1..accept_workers {
        listeners.push(TcpListener::from_std(std_listener.try_clone()?)?);
    }
    listeners.push(TcpListener::from_std(std_listener)?);

    Ok(listeners)
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
