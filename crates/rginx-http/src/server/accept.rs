use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};
use tokio_rustls::TlsAcceptor;

use rginx_core::Result;

use super::connection::{Http1ConnectionOptions, serve_connection};

pub async fn serve(
    listener: TcpListener,
    listener_id: String,
    state: crate::state::SharedState,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut connections = JoinSet::new();

    {
        let listener = listener;

        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    match changed {
                        Ok(()) if *shutdown.borrow() => {
                            tracing::info!(
                                active_connections = state.active_connection_count(),
                                "http accept loop stopping"
                            );
                            break;
                        }
                        Ok(()) => continue,
                        Err(_) => {
                            tracing::info!(
                                active_connections = state.active_connection_count(),
                                "http accept loop stopping because shutdown channel closed"
                            );
                            break;
                        }
                    }
                }
                accepted = listener.accept() => {
                    while let Some(result) = connections.try_join_next() {
                        log_connection_task_result(result);
                    }

                    let (stream, remote_addr) = accepted?;
                    let state = state.clone();
                    let shutdown = shutdown.clone();
                    let tls_acceptor: Option<TlsAcceptor> = state.tls_acceptor(&listener_id).await;
                    let current_listener = state.current_listener(&listener_id).await.expect(
                        "listener id should remain available while accept loop is running",
                    );
                    let Some(connection_guard) =
                        state.try_acquire_connection(
                            &listener_id,
                            current_listener.server.max_connections,
                        )
                    else {
                        state.record_connection_rejected(&listener_id);
                        tracing::warn!(
                            remote_addr = %remote_addr,
                            listener = %listener_id,
                            max_connections = current_listener.server.max_connections,
                            active_connections = state.active_connection_count(),
                            "rejecting downstream connection because server max_connections was reached"
                        );
                        drop(stream);
                        continue;
                    };
                    state.record_connection_accepted(&listener_id);
                    let http1 = Http1ConnectionOptions {
                        keep_alive: current_listener.server.keep_alive,
                        max_headers: current_listener.server.max_headers,
                        header_read_timeout: current_listener.server.header_read_timeout,
                        response_write_timeout: current_listener.server.response_write_timeout,
                    };
                    let connection_listener_id = listener_id.clone();

                    connections.spawn(async move {
                        serve_connection(
                            stream,
                            connection_listener_id,
                            state,
                            remote_addr,
                            shutdown,
                            tls_acceptor,
                            http1,
                            connection_guard,
                        )
                        .await;
                    });
                }
                joined = connections.join_next(), if !connections.is_empty() => {
                    if let Some(result) = joined {
                        log_connection_task_result(result);
                    }
                }
            }
        }
    }

    if !connections.is_empty() {
        tracing::info!(
            active_connections = state.active_connection_count(),
            "waiting for active connections to drain"
        );
    }

    while let Some(result) = connections.join_next().await {
        log_connection_task_result(result);
    }

    tracing::info!("http server stopped");

    Ok(())
}

fn log_connection_task_result(result: std::result::Result<(), JoinError>) {
    if let Err(error) = result {
        if error.is_panic() {
            tracing::warn!(%error, "connection task panicked");
        } else if !error.is_cancelled() {
            tracing::warn!(%error, "connection task failed to join");
        }
    }
}
