use std::convert::Infallible;
use std::net::SocketAddr;

use hyper::server::conn::{http1, http2};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rginx_core::Result;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};
use tokio_rustls::TlsAcceptor;

use crate::timeout::WriteTimeoutIo;

const ALPN_H2: &[u8] = b"h2";

#[derive(Clone, Copy)]
struct Http1ConnectionOptions {
    keep_alive: bool,
    max_headers: Option<usize>,
    header_read_timeout: Option<std::time::Duration>,
    response_write_timeout: Option<std::time::Duration>,
}

pub async fn serve(
    listener: TcpListener,
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
                    let tls_acceptor = state.tls_acceptor().await;
                    let current_config = state.current_config().await;
                    let Some(connection_guard) =
                        state.try_acquire_connection(current_config.server.max_connections)
                    else {
                        tracing::warn!(
                            remote_addr = %remote_addr,
                            max_connections = current_config.server.max_connections,
                            active_connections = state.active_connection_count(),
                            "rejecting downstream connection because server max_connections was reached"
                        );
                        drop(stream);
                        continue;
                    };
                    let http1 = Http1ConnectionOptions {
                        keep_alive: current_config.server.keep_alive,
                        max_headers: current_config.server.max_headers,
                        header_read_timeout: current_config.server.header_read_timeout,
                        response_write_timeout: current_config.server.response_write_timeout,
                    };

                    connections.spawn(async move {
                        serve_connection(
                            stream,
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

async fn serve_connection(
    stream: tokio::net::TcpStream,
    state: crate::state::SharedState,
    remote_addr: SocketAddr,
    shutdown: watch::Receiver<bool>,
    tls_acceptor: Option<TlsAcceptor>,
    http1: Http1ConnectionOptions,
    _connection_guard: crate::state::ActiveConnectionGuard,
) {
    if let Some(tls_acceptor) = tls_acceptor {
        match tls_acceptor.accept(stream).await {
            Ok(stream) => {
                if negotiated_h2(&stream) {
                    let stream = WriteTimeoutIo::new(
                        stream,
                        http1.response_write_timeout,
                        format!("downstream response to {remote_addr}"),
                    );
                    serve_h2_connection_io(TokioIo::new(stream), state, remote_addr, shutdown)
                        .await;
                } else {
                    let stream = WriteTimeoutIo::new(
                        stream,
                        http1.response_write_timeout,
                        format!("downstream response to {remote_addr}"),
                    );
                    serve_h1_connection_io(
                        TokioIo::new(stream),
                        state,
                        remote_addr,
                        shutdown,
                        http1,
                    )
                    .await;
                }
            }
            Err(error) => {
                tracing::warn!(remote_addr = %remote_addr, %error, "TLS handshake failed");
            }
        }
        return;
    }

    let stream = WriteTimeoutIo::new(
        stream,
        http1.response_write_timeout,
        format!("downstream response to {remote_addr}"),
    );
    serve_h1_connection_io(TokioIo::new(stream), state, remote_addr, shutdown, http1).await;
}

fn negotiated_h2(stream: &tokio_rustls::server::TlsStream<tokio::net::TcpStream>) -> bool {
    stream.get_ref().1.alpn_protocol() == Some(ALPN_H2)
}

async fn serve_h1_connection_io<T>(
    io: TokioIo<T>,
    state: crate::state::SharedState,
    remote_addr: SocketAddr,
    mut shutdown: watch::Receiver<bool>,
    options: Http1ConnectionOptions,
) where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service = service_fn(move |request| {
        let state = state.clone();
        async move { Ok::<_, Infallible>(crate::handler::handle(request, state, remote_addr).await) }
    });

    let mut builder = http1::Builder::new();
    builder.keep_alive(options.keep_alive);
    if let Some(max_headers) = options.max_headers {
        builder.max_headers(max_headers);
    }
    if let Some(header_read_timeout) = options.header_read_timeout {
        builder.timer(TokioTimer::new());
        builder.header_read_timeout(header_read_timeout);
    }
    let connection = builder.serve_connection(io, service).with_upgrades();
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
}

async fn serve_h2_connection_io<T>(
    io: TokioIo<T>,
    state: crate::state::SharedState,
    remote_addr: SocketAddr,
    mut shutdown: watch::Receiver<bool>,
) where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service = service_fn(move |request| {
        let state = state.clone();
        async move { Ok::<_, Infallible>(crate::handler::handle(request, state, remote_addr).await) }
    });

    let connection = http2::Builder::new(TokioExecutor::new()).serve_connection(io, service);
    tokio::pin!(connection);

    let mut draining = *shutdown.borrow();
    if draining {
        connection.as_mut().graceful_shutdown();
    }

    loop {
        tokio::select! {
            result = connection.as_mut() => {
                if let Err(error) = result {
                    tracing::warn!(
                        remote_addr = %remote_addr,
                        %error,
                        "http2 connection closed with error"
                    );
                }
                break;
            }
            changed = shutdown.changed(), if !draining => {
                match changed {
                    Ok(()) if *shutdown.borrow() => {
                        draining = true;
                        tracing::debug!(
                            remote_addr = %remote_addr,
                            "starting graceful shutdown for http2 connection"
                        );
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
