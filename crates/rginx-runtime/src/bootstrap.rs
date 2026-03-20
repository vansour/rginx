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
    let metrics = state.http.metrics();
    let current_config = state.current_config().await;
    let listener = TcpListener::bind(current_config.server.listen_addr).await?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tracing::info!(
        listen = %current_config.server.listen_addr,
        routes = current_config.routes.len(),
        "starting rginx runtime"
    );

    let mut server_task =
        tokio::spawn(rginx_http::serve(listener, state.http.clone(), shutdown_rx));
    let mut health_task = tokio::spawn(health::run(state.http.clone(), shutdown_tx.subscribe()));

    loop {
        match shutdown::wait_for_signal().await? {
            RuntimeSignal::Reload => {
                tracing::info!("reload signal received");
                match reload::reload(&state).await {
                    Ok(config) => {
                        metrics.record_config_reload("success");
                        tracing::info!(
                            listen = %config.server.listen_addr,
                            routes = config.routes.len(),
                            upstreams = config.upstreams.len(),
                            "configuration reloaded"
                        );
                    }
                    Err(error) => {
                        metrics.record_config_reload("failure");
                        tracing::warn!(%error, "configuration reload failed");
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

    match tokio::time::timeout(current_config.runtime.shutdown_timeout, async {
        (&mut server_task)
            .await
            .map_err(|error| Error::Server(format!("http task failed to join: {error}")))??;
        (&mut health_task).await.map_err(|error| {
            Error::Server(format!("active health task failed to join: {error}"))
        })?;
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
            server_task.abort();
            health_task.abort();

            if let Err(error) = server_task.await {
                if !error.is_cancelled() {
                    tracing::warn!(%error, "http task failed after abort");
                }
            }

            if let Err(error) = health_task.await {
                if !error.is_cancelled() {
                    tracing::warn!(%error, "active health task failed after abort");
                }
            }
        }
    }

    Ok(())
}
