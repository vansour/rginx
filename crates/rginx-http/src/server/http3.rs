use std::net::SocketAddr;
use std::sync::Arc;
use std::task::{Context, Poll};

use bytes::{Buf, Bytes};
use h3::quic::{RecvStream, SendStream};
use h3::server::{Connection as H3Connection, RequestResolver, RequestStream};
use http::{Response, Version};
use http_body_util::BodyExt;
use hyper::body::{Body, Frame, SizeHint};
use pin_project_lite::pin_project;
use quinn::Incoming;
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};

use rginx_core::{Error, Result};

use crate::client_ip::ConnectionPeerAddrs;
use crate::handler::{BoxError, HttpResponse};
use crate::tls::build_http3_server_config;

pin_project! {
    struct Http3RequestBody<S> {
        #[pin]
        stream: RequestStream<S, Bytes>,
        data_finished: bool,
        trailers_finished: bool,
    }
}

impl<S> Http3RequestBody<S> {
    fn new(stream: RequestStream<S, Bytes>) -> Self {
        Self { stream, data_finished: false, trailers_finished: false }
    }
}

impl<S> Body for Http3RequestBody<S>
where
    S: RecvStream,
{
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        self: std::pin::Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();

        if !*this.data_finished {
            match this.stream.as_mut().poll_recv_data(cx) {
                Poll::Ready(Ok(Some(mut chunk))) => {
                    let bytes = chunk.copy_to_bytes(chunk.remaining());
                    return Poll::Ready(Some(Ok(Frame::data(bytes))));
                }
                Poll::Ready(Ok(None)) => {
                    *this.data_finished = true;
                }
                Poll::Ready(Err(error)) => {
                    *this.data_finished = true;
                    *this.trailers_finished = true;
                    return Poll::Ready(Some(Err(stream_error(error))));
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        if !*this.trailers_finished {
            match this.stream.as_mut().poll_recv_trailers(cx) {
                Poll::Ready(Ok(Some(trailers))) => {
                    *this.trailers_finished = true;
                    return Poll::Ready(Some(Ok(Frame::trailers(trailers))));
                }
                Poll::Ready(Ok(None)) => {
                    *this.trailers_finished = true;
                }
                Poll::Ready(Err(error)) => {
                    *this.trailers_finished = true;
                    return Poll::Ready(Some(Err(stream_error(error))));
                }
                Poll::Pending => return Poll::Pending,
            }
        }

        Poll::Ready(None)
    }

    fn is_end_stream(&self) -> bool {
        self.data_finished && self.trailers_finished
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

pub async fn serve_http3(
    endpoint: quinn::Endpoint,
    listener_id: String,
    state: crate::state::SharedState,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let mut connections = JoinSet::new();
    let mut draining = *shutdown.borrow();
    if draining {
        endpoint.set_server_config(None);
    }

    loop {
        tokio::select! {
            changed = shutdown.changed(), if !draining => {
                match changed {
                    Ok(()) if *shutdown.borrow() => {
                        tracing::info!("http3 accept loop entering drain mode");
                        draining = true;
                        endpoint.set_server_config(None);
                        if connections.is_empty() {
                            break;
                        }
                    }
                    Ok(()) => continue,
                    Err(_) => {
                        tracing::info!("http3 accept loop entering drain mode because shutdown channel closed");
                        draining = true;
                        endpoint.set_server_config(None);
                        if connections.is_empty() {
                            break;
                        }
                    }
                }
            }
            accepted = endpoint.accept(), if !draining => {
                while let Some(result) = connections.try_join_next() {
                    log_connection_task_result(result);
                }

                let Some(incoming) = accepted else {
                    break;
                };

                let remote_addr = incoming.remote_address();
                let current_listener = state.current_listener(&listener_id).await.expect(
                    "listener id should remain available while http3 accept loop is running",
                );
                let Some(connection_guard) =
                    state.try_acquire_connection(&listener_id, current_listener.server.max_connections)
                else {
                    state.record_connection_rejected(&listener_id);
                    tracing::warn!(
                        remote_addr = %remote_addr,
                        listener = %listener_id,
                        max_connections = current_listener.server.max_connections,
                        active_connections = state.active_connection_count(),
                        "rejecting downstream http3 connection because server max_connections was reached"
                    );
                    incoming.refuse();
                    continue;
                };
                state.record_connection_accepted(&listener_id);

                let state = state.clone();
                let shutdown = shutdown.clone();
                let listener_id = listener_id.clone();
                connections.spawn(async move {
                    serve_http3_connection(
                        incoming,
                        listener_id,
                        state,
                        remote_addr,
                        shutdown,
                        connection_guard,
                    )
                    .await
                });
            }
            joined = connections.join_next(), if !connections.is_empty() => {
                if let Some(result) = joined {
                    log_connection_task_result(result);
                }
                if draining && connections.is_empty() {
                    break;
                }
            }
            else => {
                if draining || connections.is_empty() {
                    break;
                }
            }
        }
    }

    while let Some(result) = connections.join_next().await {
        log_connection_task_result(result);
    }

    endpoint.close(quinn::VarInt::from_u32(0), b"shutdown");
    endpoint.wait_idle().await;

    tracing::info!("http3 server stopped");
    Ok(())
}

pub fn bind_http3_endpoint(
    listener: &rginx_core::Listener,
    default_vhost: &rginx_core::VirtualHost,
    vhosts: &[rginx_core::VirtualHost],
) -> Result<Option<quinn::Endpoint>> {
    let listen_addr = match listener.http3.as_ref() {
        Some(http3) => http3.listen_addr,
        None => return Ok(None),
    };
    let socket = std::net::UdpSocket::bind(listen_addr).map_err(Error::Io)?;
    socket.set_nonblocking(true).map_err(Error::Io)?;
    bind_http3_endpoint_with_socket(listener, default_vhost, vhosts, socket).map(Some)
}

pub fn bind_http3_endpoint_with_socket(
    listener: &rginx_core::Listener,
    default_vhost: &rginx_core::VirtualHost,
    vhosts: &[rginx_core::VirtualHost],
    socket: std::net::UdpSocket,
) -> Result<quinn::Endpoint> {
    let server_config = build_http3_server_config(
        listener.server.tls.as_ref(),
        listener.server.default_certificate.as_deref(),
        listener.tls_enabled(),
        default_vhost,
        vhosts,
    )?
    .ok_or_else(|| {
        Error::Config("http3 listener requires downstream TLS termination".to_string())
    })?;

    let quic_config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(server_config).map_err(|error| {
            Error::Server(format!("failed to build quic server config for http3 listener: {error}"))
        })?,
    ));
    let runtime = quinn::default_runtime()
        .ok_or_else(|| Error::Server("no async runtime found for http3 endpoint".to_string()))?;
    quinn::Endpoint::new(quinn::EndpointConfig::default(), Some(quic_config), socket, runtime)
        .map_err(Error::Io)
}

async fn serve_http3_connection(
    incoming: Incoming,
    listener_id: String,
    state: crate::state::SharedState,
    remote_addr: SocketAddr,
    mut shutdown: watch::Receiver<bool>,
    _connection_guard: crate::state::ActiveConnectionGuard,
) -> Result<()> {
    let connection = incoming
        .await
        .map_err(|error| Error::Server(format!("http3 connection handshake failed: {error}")))?;
    let mut request_tasks = JoinSet::new();
    let h3_connection = h3_quinn::Connection::new(connection);
    let mut h3 = H3Connection::new(h3_connection).await.map_err(|error| {
        Error::Server(format!("failed to initialize http3 connection: {error}"))
    })?;

    let connection_addrs = Arc::new(ConnectionPeerAddrs {
        socket_peer_addr: remote_addr,
        proxy_protocol_source_addr: None,
        tls_client_identity: None,
        tls_version: Some("TLS1.3".to_string()),
        tls_alpn: Some("h3".to_string()),
    });

    let mut draining = *shutdown.borrow();
    if draining {
        let _ = h3.shutdown(0).await;
    }

    loop {
        tokio::select! {
            changed = shutdown.changed(), if !draining => {
                match changed {
                    Ok(()) if *shutdown.borrow() => {
                        draining = true;
                        let _ = h3.shutdown(0).await;
                    }
                    Ok(()) => {}
                    Err(_) => {
                        draining = true;
                        let _ = h3.shutdown(0).await;
                    }
                }
            }
            accepted = h3.accept(), if !draining => {
                match accepted {
                    Ok(Some(resolver)) => {
                        let state = state.clone();
                        let listener_id = listener_id.clone();
                        let connection_addrs = connection_addrs.clone();
                        request_tasks.spawn(async move {
                            serve_http3_request(resolver, listener_id, state, connection_addrs).await
                        });
                    }
                    Ok(None) => break,
                    Err(error) => {
                        return Err(Error::Server(format!("http3 request accept failed: {error}")));
                    }
                }
            }
            joined = request_tasks.join_next(), if !request_tasks.is_empty() => {
                if let Some(result) = joined {
                    log_request_task_result(result);
                }
                if draining && request_tasks.is_empty() {
                    break;
                }
            }
            else => {
                if request_tasks.is_empty() {
                    break;
                }
            }
        }
    }

    while let Some(result) = request_tasks.join_next().await {
        log_request_task_result(result);
    }

    Ok(())
}

async fn serve_http3_request(
    resolver: RequestResolver<h3_quinn::Connection, Bytes>,
    listener_id: String,
    state: crate::state::SharedState,
    connection_addrs: Arc<ConnectionPeerAddrs>,
) -> Result<()> {
    let (request, stream) = resolver
        .resolve_request()
        .await
        .map_err(|error| Error::Server(format!("failed to resolve http3 request: {error}")))?;
    let (send_stream, recv_stream) = stream.split();
    let mut request =
        request.map(|()| crate::handler::boxed_body(Http3RequestBody::new(recv_stream)));
    *request.version_mut() = Version::HTTP_3;

    let response = crate::handler::handle(request, state, connection_addrs, &listener_id).await;
    send_http3_response(send_stream, response).await
}

async fn send_http3_response<S>(
    mut stream: RequestStream<S, Bytes>,
    response: HttpResponse,
) -> Result<()>
where
    S: SendStream<Bytes>,
{
    let (parts, mut body) = response.into_parts();
    let mut response_builder = Response::builder().status(parts.status);
    for (name, value) in &parts.headers {
        response_builder = response_builder.header(name, value);
    }
    let response = response_builder.body(()).map_err(|error| {
        Error::Server(format!("failed to build http3 response headers: {error}"))
    })?;
    stream.send_response(response).await.map_err(|error| {
        Error::Server(format!("failed to send http3 response headers: {error}"))
    })?;

    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| {
            Error::Server(format!("failed to read response body frame for http3: {error}"))
        })?;
        match frame.into_data() {
            Ok(data) => {
                if !data.is_empty() {
                    tracing::debug!(len = data.len(), "http3 sending response data frame");
                    stream.send_data(data).await.map_err(|error| {
                        Error::Server(format!("failed to send http3 response body: {error}"))
                    })?;
                }
            }
            Err(frame) => {
                if let Ok(trailers) = frame.into_trailers() {
                    tracing::debug!(count = trailers.len(), "http3 sending response trailers");
                    stream.send_trailers(trailers).await.map_err(|error| {
                        Error::Server(format!("failed to send http3 response trailers: {error}"))
                    })?;
                }
            }
        }
    }

    stream
        .finish()
        .await
        .map_err(|error| Error::Server(format!("failed to finish http3 response stream: {error}")))
}

fn log_connection_task_result(result: std::result::Result<Result<()>, JoinError>) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(%error, "http3 connection task failed");
        }
        Err(error) if error.is_panic() => {
            tracing::warn!(%error, "http3 connection task panicked");
        }
        Err(error) if !error.is_cancelled() => {
            tracing::warn!(%error, "http3 connection task failed to join");
        }
        Err(_) => {}
    }
}

fn log_request_task_result(result: std::result::Result<Result<()>, JoinError>) {
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::warn!(%error, "http3 request task failed");
        }
        Err(error) if error.is_panic() => {
            tracing::warn!(%error, "http3 request task panicked");
        }
        Err(error) if !error.is_cancelled() => {
            tracing::warn!(%error, "http3 request task failed to join");
        }
        Err(_) => {}
    }
}

fn stream_error(error: impl std::fmt::Display) -> BoxError {
    std::io::Error::other(format!("{error}")).into()
}
