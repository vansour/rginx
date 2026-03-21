use std::convert::Infallible;
use std::net::SocketAddr;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use rginx_core::Result;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};
use tokio_rustls::TlsAcceptor;

pub async fn serve(
    listener: TcpListener,
    state: crate::state::SharedState,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut connections = JoinSet::new();
    let metrics = state.metrics();

    {
        let listener = listener;

        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    match changed {
                        Ok(()) if *shutdown.borrow() => {
                            tracing::info!(
                                active_connections = connections.len(),
                                "http accept loop stopping"
                            );
                            break;
                        }
                        Ok(()) => continue,
                        Err(_) => {
                            tracing::info!(
                                active_connections = connections.len(),
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
                    let current_config = state.current_config().await;
                    let max_connections = current_config.server.max_connections;
                    if max_connections.is_some_and(|limit| metrics.active_connections() >= limit as u64)
                    {
                        tracing::warn!(
                            remote_addr = %remote_addr,
                            max_connections = max_connections.unwrap_or_default(),
                            active_connections = metrics.active_connections(),
                            "rejecting connection because the server connection limit is reached"
                        );
                        drop(stream);
                        continue;
                    }

                    let state = state.clone();
                    let metrics = metrics.clone();
                    let shutdown = shutdown.clone();
                    metrics.increment_active_connections();
                    let tls_acceptor = state.tls_acceptor().await;
                    let keep_alive = current_config.server.keep_alive;
                    let max_headers = current_config.server.max_headers;
                    let header_read_timeout = current_config.server.header_read_timeout;

                    connections.spawn(async move {
                        serve_connection(
                            stream,
                            state,
                            metrics,
                            remote_addr,
                            shutdown,
                            tls_acceptor,
                            keep_alive,
                            max_headers,
                            header_read_timeout,
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
            active_connections = connections.len(),
            "waiting for active connections to drain"
        );
    }

    while let Some(result) = connections.join_next().await {
        log_connection_task_result(result);
    }

    tracing::info!("http server stopped");

    Ok(())
}

async fn serve_connection(
    stream: tokio::net::TcpStream,
    state: crate::state::SharedState,
    metrics: crate::metrics::Metrics,
    remote_addr: SocketAddr,
    shutdown: watch::Receiver<bool>,
    tls_acceptor: Option<TlsAcceptor>,
    keep_alive: bool,
    max_headers: Option<usize>,
    header_read_timeout: Option<std::time::Duration>,
) {
    if let Some(tls_acceptor) = tls_acceptor {
        match tls_acceptor.accept(stream).await {
            Ok(stream) => {
                serve_connection_io(
                    TokioIo::new(stream),
                    state,
                    metrics.clone(),
                    remote_addr,
                    shutdown,
                    keep_alive,
                    max_headers,
                    header_read_timeout,
                )
                .await;
            }
            Err(error) => {
                tracing::warn!(remote_addr = %remote_addr, %error, "TLS handshake failed");
                metrics.decrement_active_connections();
            }
        }
        return;
    }

    serve_connection_io(
        TokioIo::new(stream),
        state,
        metrics,
        remote_addr,
        shutdown,
        keep_alive,
        max_headers,
        header_read_timeout,
    )
    .await;
}

async fn serve_connection_io<T>(
    io: TokioIo<T>,
    state: crate::state::SharedState,
    metrics: crate::metrics::Metrics,
    remote_addr: SocketAddr,
    mut shutdown: watch::Receiver<bool>,
    keep_alive: bool,
    max_headers: Option<usize>,
    header_read_timeout: Option<std::time::Duration>,
) where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service = service_fn(move |request| {
        let state = state.clone();
        async move { Ok::<_, Infallible>(crate::handler::handle(request, state, remote_addr).await) }
    });

    let mut builder = http1::Builder::new();
    builder.keep_alive(keep_alive);
    if let Some(max_headers) = max_headers {
        builder.max_headers(max_headers);
    }
    if let Some(header_read_timeout) = header_read_timeout {
        builder.timer(TokioTimer::new());
        builder.header_read_timeout(header_read_timeout);
    }
    let connection = builder.serve_connection(io, service);
    tokio::pin!(connection);

    let mut draining = *shutdown.borrow();
    if draining {
        connection.as_mut().graceful_shutdown();
    }

    loop {
        tokio::select! {
            result = connection.as_mut() => {
                if let Err(error) = result {
                    tracing::warn!(remote_addr = %remote_addr, %error, "connection closed with error");
                }
                break;
            }
            changed = shutdown.changed(), if !draining => {
                match changed {
                    Ok(()) if *shutdown.borrow() => {
                        draining = true;
                        tracing::debug!(remote_addr = %remote_addr, "starting graceful shutdown for connection");
                        connection.as_mut().graceful_shutdown();
                    }
                    Ok(()) => {}
                    Err(_) => {
                        draining = true;
                        connection.as_mut().graceful_shutdown();
                    }
                }
            }
        }
    }

    metrics.decrement_active_connections();
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
