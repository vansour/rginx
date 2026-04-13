use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use aws_lc_rs::{hkdf, hmac, rand, rand::SecureRandom};
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
use sha2::{Digest, Sha256};

use crate::client_ip::{ConnectionPeerAddrs, TlsClientIdentity};
use crate::handler::{BoxError, HttpResponse};
use crate::tls::build_http3_server_config;

const HTTP3_HOST_KEY_BYTES: usize = 64;
const HTTP3_ACTIVE_CONNECTION_ID_LIMIT_NO_MIGRATION: u32 = 2;
const HTTP3_ACTIVE_CONNECTION_ID_LIMIT_MIGRATION: u32 = 5;

pin_project! {
    struct Http3RequestBody<S> {
        #[pin]
        stream: RequestStream<S, Bytes>,
        state: crate::state::SharedState,
        listener_id: String,
        data_finished: bool,
        trailers_finished: bool,
        error_recorded: bool,
    }
}

impl<S> Http3RequestBody<S> {
    fn new(
        stream: RequestStream<S, Bytes>,
        state: crate::state::SharedState,
        listener_id: String,
    ) -> Self {
        Self {
            stream,
            state,
            listener_id,
            data_finished: false,
            trailers_finished: false,
            error_recorded: false,
        }
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
                    if !*this.error_recorded {
                        this.state
                            .record_http3_request_body_stream_error(this.listener_id.as_str());
                        *this.error_recorded = true;
                    }
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
                    if !*this.error_recorded {
                        this.state
                            .record_http3_request_body_stream_error(this.listener_id.as_str());
                        *this.error_recorded = true;
                    }
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

                let current_listener = state.current_listener(&listener_id).await.expect(
                    "listener id should remain available while http3 accept loop is running",
                );
                if current_listener.http3.as_ref().is_some_and(|http3| http3.retry)
                    && !incoming.remote_address_validated()
                    && incoming.may_retry()
                {
                    let remote_addr = incoming.remote_address();
                    state.record_http3_retry_issued(&listener_id);
                    tracing::info!(
                        remote_addr = %remote_addr,
                        listener = %listener_id,
                        "http3 issuing retry to validate client address"
                    );
                    if let Err(error) = incoming.retry() {
                        state.record_http3_retry_failed(&listener_id);
                        tracing::warn!(
                            remote_addr = %remote_addr,
                            listener = %listener_id,
                            %error,
                            "http3 retry failed"
                        );
                    }
                    continue;
                }

                let remote_addr = incoming.remote_address();
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
    let rustls_server_config = build_http3_server_config(
        listener.server.tls.as_ref(),
        listener.server.default_certificate.as_deref(),
        listener.tls_enabled(),
        default_vhost,
        vhosts,
        listener.server.max_request_body_bytes,
        listener.http3.as_ref().is_some_and(|http3| http3.early_data_enabled),
    )?
    .ok_or_else(|| {
        Error::Config("http3 listener requires downstream TLS termination".to_string())
    })?;

    let host_key_material = load_or_create_http3_host_key(listener.http3.as_ref())?;
    let endpoint_config =
        build_http3_endpoint_config(listener.http3.as_ref(), host_key_material.as_deref())?;
    let mut quic_config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(rustls_server_config).map_err(
            |error| {
                Error::Server(format!(
                    "failed to build quic server config for http3 listener: {error}"
                ))
            },
        )?,
    ));
    apply_http3_server_runtime(
        listener.http3.as_ref(),
        host_key_material.as_deref(),
        &mut quic_config,
    )?;
    let runtime = quinn::default_runtime()
        .ok_or_else(|| Error::Server("no async runtime found for http3 endpoint".to_string()))?;
    quinn::Endpoint::new(endpoint_config, Some(quic_config), socket, runtime).map_err(Error::Io)
}

fn apply_http3_server_runtime(
    http3: Option<&rginx_core::ListenerHttp3>,
    host_key_material: Option<&[u8]>,
    quic_config: &mut quinn::ServerConfig,
) -> Result<()> {
    let Some(http3) = http3 else {
        return Ok(());
    };

    let mut transport = quinn::TransportConfig::default();
    transport.max_concurrent_bidi_streams(
        quinn::VarInt::try_from(http3.max_concurrent_streams as u64).map_err(|_| {
            Error::Config(format!(
                "http3 max_concurrent_streams `{}` exceeds QUIC transport limits",
                http3.max_concurrent_streams
            ))
        })?,
    );
    transport.stream_receive_window(
        quinn::VarInt::try_from(http3.stream_buffer_size as u64).map_err(|_| {
            Error::Config(format!(
                "http3 stream_buffer_size `{}` exceeds QUIC transport limits",
                http3.stream_buffer_size
            ))
        })?,
    );
    let receive_window = (http3.max_concurrent_streams as u128)
        .checked_mul(http3.stream_buffer_size as u128)
        .ok_or_else(|| {
            Error::Config(format!(
                "http3 receive window derived from max_concurrent_streams={} and stream_buffer_size={} exceeds platform limits",
                http3.max_concurrent_streams, http3.stream_buffer_size
            ))
        })?;
    let receive_window = u64::try_from(receive_window).map_err(|_| {
        Error::Config(format!(
            "http3 receive window derived from max_concurrent_streams={} and stream_buffer_size={} exceeds platform limits",
            http3.max_concurrent_streams, http3.stream_buffer_size
        ))
    })?;
    transport.receive_window(quinn::VarInt::try_from(receive_window).map_err(|_| {
        Error::Config(format!(
            "http3 receive window derived from max_concurrent_streams={} and stream_buffer_size={} exceeds QUIC transport limits",
            http3.max_concurrent_streams, http3.stream_buffer_size
        ))
    })?);
    transport.enable_segmentation_offload(http3.gso);
    quic_config.transport_config(Arc::new(transport));
    quic_config.migration(
        http3.active_connection_id_limit != HTTP3_ACTIVE_CONNECTION_ID_LIMIT_NO_MIGRATION,
    );

    if let Some(host_key_material) = host_key_material {
        quic_config
            .token_key(Arc::new(hkdf::Salt::new(hkdf::HKDF_SHA256, &[]).extract(
                &derive_labeled_key_material(host_key_material, b"rginx-http3-token-key"),
            )));
    }

    Ok(())
}

fn build_http3_endpoint_config(
    http3: Option<&rginx_core::ListenerHttp3>,
    host_key_material: Option<&[u8]>,
) -> Result<quinn::EndpointConfig> {
    let mut endpoint_config = quinn::EndpointConfig::default();
    let Some(http3) = http3 else {
        return Ok(endpoint_config);
    };

    if let Some(host_key_material) = host_key_material {
        endpoint_config.reset_key(Arc::new(hmac::Key::new(
            hmac::HMAC_SHA256,
            &derive_labeled_key_material(host_key_material, b"rginx-http3-reset-key"),
        )));
    }

    match http3.active_connection_id_limit {
        HTTP3_ACTIVE_CONNECTION_ID_LIMIT_NO_MIGRATION => {
            endpoint_config
                .cid_generator(|| Box::new(quinn_proto::RandomConnectionIdGenerator::new(0)));
        }
        HTTP3_ACTIVE_CONNECTION_ID_LIMIT_MIGRATION => {
            if let Some(host_key_material) = host_key_material {
                let key = derive_hashed_connection_id_key(host_key_material);
                endpoint_config.cid_generator(move || {
                    Box::new(quinn_proto::HashedConnectionIdGenerator::from_key(key))
                });
            }
        }
        unsupported => {
            return Err(Error::Config(format!(
                "http3 active_connection_id_limit `{unsupported}` is not supported by the current QUIC stack"
            )));
        }
    }

    Ok(endpoint_config)
}

fn load_or_create_http3_host_key(
    http3: Option<&rginx_core::ListenerHttp3>,
) -> Result<Option<Vec<u8>>> {
    let Some(path) = http3.and_then(|http3| http3.host_key_path.as_deref()) else {
        return Ok(None);
    };

    Ok(Some(load_or_create_host_key_material(path)?))
}

fn load_or_create_host_key_material(path: &Path) -> Result<Vec<u8>> {
    match std::fs::read(path) {
        Ok(bytes) => validate_host_key_material(path, bytes),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            create_host_key_material(path)
        }
        Err(error) => Err(Error::Io(error)),
    }
}

fn validate_host_key_material(path: &Path, bytes: Vec<u8>) -> Result<Vec<u8>> {
    if bytes.len() != HTTP3_HOST_KEY_BYTES {
        return Err(Error::Config(format!(
            "http3 host_key_path `{}` must contain exactly {} bytes",
            path.display(),
            HTTP3_HOST_KEY_BYTES
        )));
    }

    Ok(bytes)
}

fn create_host_key_material(path: &Path) -> Result<Vec<u8>> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let mut bytes = vec![0u8; HTTP3_HOST_KEY_BYTES];
    rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::Server("failed to generate http3 host key material".to_string()))?;

    use std::io::Write as _;

    let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return validate_host_key_material(path, std::fs::read(path).map_err(Error::Io)?);
        }
        Err(error) => return Err(Error::Io(error)),
    };
    file.write_all(&bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    file.flush().map_err(Error::Io)?;
    file.sync_all().map_err(Error::Io)?;

    Ok(bytes)
}

fn derive_labeled_key_material(host_key_material: &[u8], label: &[u8]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(label);
    digest.update(host_key_material);
    digest.finalize().into()
}

fn derive_hashed_connection_id_key(host_key_material: &[u8]) -> u64 {
    let digest = derive_labeled_key_material(host_key_material, b"rginx-http3-cid-key");
    u64::from_be_bytes(digest[..8].try_into().expect("sha256 digest should contain 8 bytes"))
}

async fn serve_http3_connection(
    incoming: Incoming,
    listener_id: String,
    state: crate::state::SharedState,
    remote_addr: SocketAddr,
    mut shutdown: watch::Receiver<bool>,
    _connection_guard: crate::state::ActiveConnectionGuard,
) -> Result<()> {
    let current_listener = state
        .current_listener(&listener_id)
        .await
        .expect("listener id should remain available while http3 connection is running");
    let _http3_connection_guard = state.retain_http3_connection(&listener_id)?;
    let mtls_configured =
        current_listener.server.tls.as_ref().and_then(|tls| tls.client_auth.as_ref()).is_some();
    let early_data_enabled =
        current_listener.http3.as_ref().is_some_and(|http3| http3.early_data_enabled);
    let connecting = match incoming.accept() {
        Ok(connecting) => connecting,
        Err(error) => {
            let reason = super::connection::classify_tls_handshake_failure(&error);
            state.record_tls_handshake_failure(&listener_id, reason);
            tracing::warn!(
                remote_addr = %remote_addr,
                listener = %listener_id,
                tls_handshake_failure = reason.as_str(),
                %error,
                "http3 TLS handshake failed"
            );
            return Ok(());
        }
    };
    let (connection, handshake_complete) = if early_data_enabled && !mtls_configured {
        match connecting.into_0rtt() {
            Ok((connection, zero_rtt_accepted)) => {
                let handshake_complete = Arc::new(AtomicBool::new(false));
                let observed = handshake_complete.clone();
                tokio::spawn(async move {
                    let _ = zero_rtt_accepted.await;
                    observed.store(true, Ordering::Release);
                });
                (connection, handshake_complete)
            }
            Err(connecting) => {
                let connection = match connecting.await {
                    Ok(connection) => connection,
                    Err(error) => {
                        let reason = super::connection::classify_tls_handshake_failure(&error);
                        state.record_tls_handshake_failure(&listener_id, reason);
                        tracing::warn!(
                            remote_addr = %remote_addr,
                            listener = %listener_id,
                            tls_handshake_failure = reason.as_str(),
                            %error,
                            "http3 TLS handshake failed"
                        );
                        return Ok(());
                    }
                };
                let handshake_complete = Arc::new(AtomicBool::new(true));
                (connection, handshake_complete)
            }
        }
    } else {
        let connection = match connecting.await {
            Ok(connection) => connection,
            Err(error) => {
                let reason = super::connection::classify_tls_handshake_failure(&error);
                state.record_tls_handshake_failure(&listener_id, reason);
                tracing::warn!(
                    remote_addr = %remote_addr,
                    listener = %listener_id,
                    tls_handshake_failure = reason.as_str(),
                    %error,
                    "http3 TLS handshake failed"
                );
                return Ok(());
            }
        };
        let handshake_complete = Arc::new(AtomicBool::new(true));
        (connection, handshake_complete)
    };
    let transport_connection = connection.clone();
    let tls_client_identity = extract_http3_tls_client_identity(&transport_connection);
    let tls_alpn = http3_tls_alpn_protocol(&transport_connection);
    let mut request_tasks = JoinSet::new();
    let h3_connection = h3_quinn::Connection::new(connection);
    let mut h3 = H3Connection::new(h3_connection).await.map_err(|error| {
        Error::Server(format!("failed to initialize http3 connection: {error}"))
    })?;
    if mtls_configured {
        state.record_mtls_handshake_success(&listener_id, tls_client_identity.is_some());
    }

    let connection_addrs = Arc::new(ConnectionPeerAddrs {
        socket_peer_addr: remote_addr,
        proxy_protocol_source_addr: None,
        tls_client_identity,
        tls_version: Some("TLS1.3".to_string()),
        tls_alpn: tls_alpn.or_else(|| Some("h3".to_string())),
        early_data: false,
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
                        let handshake_complete = handshake_complete.clone();
                        request_tasks.spawn(async move {
                            let _request_stream_guard =
                                state.retain_http3_request_stream(&listener_id)?;
                            let mut connection_addrs = connection_addrs.as_ref().clone();
                            connection_addrs.early_data = !handshake_complete.load(Ordering::Acquire);
                            serve_http3_request(
                                resolver,
                                listener_id,
                                state,
                                Arc::new(connection_addrs),
                            )
                            .await
                        });
                    }
                    Ok(None) => break,
                    Err(error) => {
                        state.record_http3_request_accept_error(&listener_id);
                        return Err(Error::Server(format!("http3 request accept failed: {error}")));
                    }
                }
            }
            joined = request_tasks.join_next(), if !request_tasks.is_empty() => {
                if let Some(result) = joined {
                    log_request_task_result(result);
                }
                if draining && request_tasks.is_empty() {
                    // Give Quinn a brief window to flush the final response frames before
                    // dropping the drained connection.
                    tokio::time::sleep(Duration::from_millis(50)).await;
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

    let close_reason = transport_connection.closed().await;
    state.record_http3_connection_close(&listener_id, close_reason);

    Ok(())
}

fn extract_http3_tls_client_identity(connection: &quinn::Connection) -> Option<TlsClientIdentity> {
    let certificates = connection
        .peer_identity()?
        .downcast::<Vec<rustls::pki_types::CertificateDer<'static>>>()
        .ok()?;
    Some(super::connection::parse_tls_client_identity(
        certificates.iter().map(|certificate| certificate.as_ref()),
    ))
}

fn http3_tls_alpn_protocol(connection: &quinn::Connection) -> Option<String> {
    connection
        .handshake_data()?
        .downcast::<quinn::crypto::rustls::HandshakeData>()
        .ok()
        .and_then(|data| data.protocol)
        .map(|protocol| String::from_utf8_lossy(&protocol).into_owned())
}

async fn serve_http3_request(
    resolver: RequestResolver<h3_quinn::Connection, Bytes>,
    listener_id: String,
    state: crate::state::SharedState,
    connection_addrs: Arc<ConnectionPeerAddrs>,
) -> Result<()> {
    let (request, stream) = match resolver.resolve_request().await {
        Ok(result) => result,
        Err(error) => {
            state.record_http3_request_resolve_error(&listener_id);
            return Err(Error::Server(format!("failed to resolve http3 request: {error}")));
        }
    };
    let (send_stream, recv_stream) = stream.split();
    let mut request = request.map(|()| {
        crate::handler::boxed_body(Http3RequestBody::new(
            recv_stream,
            state.clone(),
            listener_id.clone(),
        ))
    });
    *request.version_mut() = Version::HTTP_3;

    let response =
        crate::handler::handle(request, state.clone(), connection_addrs, &listener_id).await;
    send_http3_response(send_stream, response, &state, &listener_id).await
}

async fn send_http3_response<S>(
    mut stream: RequestStream<S, Bytes>,
    response: HttpResponse,
    state: &crate::state::SharedState,
    listener_id: &str,
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
        state.record_http3_response_stream_error(listener_id);
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
                        state.record_http3_response_stream_error(listener_id);
                        Error::Server(format!("failed to send http3 response body: {error}"))
                    })?;
                }
            }
            Err(frame) => {
                if let Ok(trailers) = frame.into_trailers() {
                    tracing::debug!(count = trailers.len(), "http3 sending response trailers");
                    stream.send_trailers(trailers).await.map_err(|error| {
                        state.record_http3_response_stream_error(listener_id);
                        Error::Server(format!("failed to send http3 response trailers: {error}"))
                    })?;
                }
            }
        }
    }

    stream.finish().await.map_err(|error| {
        state.record_http3_response_stream_error(listener_id);
        Error::Server(format!("failed to finish http3 response stream: {error}"))
    })
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
