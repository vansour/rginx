use rginx_core::{ConfigSnapshot, Error, Result};
use tokio::net::TcpListener;
use tokio::sync::watch;

use crate::shutdown;
use crate::state::RuntimeState;

pub async fn run(config: ConfigSnapshot) -> Result<()> {
    let state = RuntimeState::new(config);
    let listener = TcpListener::bind(state.config.server.listen_addr).await?;
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    tracing::info!(
        listen = %state.config.server.listen_addr,
        routes = state.config.routes.len(),
        "starting rginx runtime"
    );

    let server_task = tokio::spawn(rginx_http::serve(listener, state.config.clone(), shutdown_rx));

    shutdown::wait_for_signal().await?;

    tracing::info!(
        timeout_secs = state.config.runtime.shutdown_timeout.as_secs(),
        "graceful shutdown requested"
    );
    let _ = shutdown_tx.send(true);

    match tokio::time::timeout(state.config.runtime.shutdown_timeout, server_task).await {
        Ok(join_result) => {
            join_result
                .map_err(|error| Error::Server(format!("http task failed to join: {error}")))??;
        }
        Err(_) => {
            tracing::warn!("shutdown timeout reached before accept loop exited");
        }
    }

    Ok(())
}
