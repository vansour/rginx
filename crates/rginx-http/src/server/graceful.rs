use std::convert::Infallible;
use std::sync::Arc;

use hyper::server::conn::{http1, http2};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;

use crate::client_ip::ConnectionPeerAddrs;

use super::connection::Http1ConnectionOptions;

pub(super) async fn serve_h1_connection_io<T>(
    io: TokioIo<T>,
    listener_id: String,
    state: crate::state::SharedState,
    connection_addrs: Arc<ConnectionPeerAddrs>,
    mut shutdown: watch::Receiver<bool>,
    options: Http1ConnectionOptions,
) where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service_connection_addrs = connection_addrs.clone();
    let service = service_fn(move |request| {
        let state = state.clone();
        let listener_id = listener_id.clone();
        let connection_addrs = service_connection_addrs.clone();
        async move {
            Ok::<_, Infallible>(
                crate::handler::handle(request, state, connection_addrs, &listener_id).await,
            )
        }
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
                    tracing::warn!(remote_addr = %connection_addrs.socket_peer_addr, %error, "connection closed with error");
                }
                break;
            }
            changed = shutdown.changed(), if !draining => {
                match changed {
                    Ok(()) if *shutdown.borrow() => {
                        draining = true;
                        tracing::debug!(remote_addr = %connection_addrs.socket_peer_addr, "starting graceful shutdown for connection");
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

pub(super) async fn serve_h2_connection_io<T>(
    io: TokioIo<T>,
    listener_id: String,
    state: crate::state::SharedState,
    connection_addrs: Arc<ConnectionPeerAddrs>,
    mut shutdown: watch::Receiver<bool>,
) where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service_connection_addrs = connection_addrs.clone();
    let service = service_fn(move |request| {
        let state = state.clone();
        let listener_id = listener_id.clone();
        let connection_addrs = service_connection_addrs.clone();
        async move {
            Ok::<_, Infallible>(
                crate::handler::handle(request, state, connection_addrs, &listener_id).await,
            )
        }
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
                        remote_addr = %connection_addrs.socket_peer_addr,
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
                            remote_addr = %connection_addrs.socket_peer_addr,
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
