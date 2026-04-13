use bytes::{Buf, Bytes};
use h3::client;
use http::{HeaderMap, Request, Response};
use hyper::body::{Frame, SizeHint};
use std::collections::HashMap;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::{Mutex, Notify, mpsc};
use tokio::task::JoinHandle;

use crate::handler::{BoxError, HttpBody, boxed_body};

use super::*;

type H3SendRequest = h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>;
type H3RequestStream = h3::client::RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>;

#[derive(Clone)]
pub(crate) struct Http3Client {
    client_config: quinn::ClientConfig,
    connect_timeout: Duration,
    endpoints: Arc<Http3ClientEndpoints>,
    sessions: Arc<Mutex<HashMap<Http3SessionKey, Http3SessionEntry>>>,
}

#[derive(Default)]
struct Http3ClientEndpoints {
    ipv4: tokio::sync::Mutex<Option<quinn::Endpoint>>,
    ipv6: tokio::sync::Mutex<Option<quinn::Endpoint>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Http3SessionKey {
    remote_addr: SocketAddr,
    server_name: String,
}

struct Http3Session {
    sender: Mutex<H3SendRequest>,
    closed: Arc<AtomicBool>,
    driver_task: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
enum Http3SessionEntry {
    Ready(Arc<Http3Session>),
    Pending(Arc<Notify>),
}

impl Http3Session {
    fn new(sender: H3SendRequest) -> Self {
        Self {
            sender: Mutex::new(sender),
            closed: Arc::new(AtomicBool::new(false)),
            driver_task: Mutex::new(None),
        }
    }

    async fn sender(&self) -> H3SendRequest {
        self.sender.lock().await.clone()
    }

    async fn set_driver_task(&self, task: JoinHandle<()>) {
        *self.driver_task.lock().await = Some(task);
    }

    fn mark_closed(&self) {
        self.closed.store(true, Ordering::Release);
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

impl Drop for Http3Session {
    fn drop(&mut self) {
        if let Ok(mut task) = self.driver_task.try_lock()
            && let Some(task) = task.take()
            && !task.is_finished()
        {
            task.abort();
        }
    }
}

impl Http3Client {
    pub(super) fn new(client_config: quinn::ClientConfig, connect_timeout: Duration) -> Self {
        Self {
            client_config,
            connect_timeout,
            endpoints: Arc::new(Http3ClientEndpoints::default()),
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(super) async fn request(
        &self,
        upstream: &Upstream,
        peer: &UpstreamPeer,
        request: Request<HttpBody>,
    ) -> Result<Response<HttpBody>, Error> {
        let remote_addr = resolve_peer_socket_addr(peer)?;
        let server_name = server_name_for_peer(upstream, peer)?;
        let session =
            self.session_for(Http3SessionKey { remote_addr, server_name }, &peer.url).await?;
        let mut send_request = session.sender().await;

        let (parts, mut body) = request.into_parts();
        let mut request_builder =
            Request::builder().method(parts.method).uri(parts.uri).version(Version::HTTP_3);
        for (name, value) in &parts.headers {
            request_builder = request_builder.header(name, value);
        }
        let request = request_builder.body(()).map_err(|error| {
            Error::Server(format!("failed to build upstream http3 request: {error}"))
        })?;

        let mut request_stream = send_request.send_request(request).await.map_err(|error| {
            session.mark_closed();
            Error::Server(format!(
                "failed to send upstream http3 request headers to `{}`: {error}",
                peer.url
            ))
        })?;

        let mut finalized = false;
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|error| {
                Error::Server(format!(
                    "failed to read downstream request body for upstream http3 `{}`: {error}",
                    peer.url
                ))
            })?;
            match frame.into_data() {
                Ok(data) => {
                    if !data.is_empty() {
                        request_stream.send_data(data).await.map_err(|error| {
                            session.mark_closed();
                            Error::Server(format!(
                                "failed to send upstream http3 request body to `{}`: {error}",
                                peer.url
                            ))
                        })?;
                    }
                }
                Err(frame) => {
                    if let Ok(trailers) = frame.into_trailers() {
                        request_stream.send_trailers(trailers).await.map_err(|error| {
                            session.mark_closed();
                            Error::Server(format!(
                                "failed to send upstream http3 request trailers to `{}`: {error}",
                                peer.url
                            ))
                        })?;
                        finalized = true;
                    }
                }
            }
        }
        if !finalized {
            request_stream.finish().await.map_err(|error| {
                session.mark_closed();
                Error::Server(format!(
                    "failed to finish upstream http3 request to `{}`: {error}",
                    peer.url
                ))
            })?;
        }

        let response = request_stream.recv_response().await.map_err(|error| {
            session.mark_closed();
            Error::Server(format!(
                "failed to receive upstream http3 response headers from `{}`: {error}",
                peer.url
            ))
        })?;

        let (parts, _) = response.into_parts();
        let size_hint = response_size_hint(&parts.headers);
        let body = streaming_response_body(request_stream, session, peer.url.clone(), size_hint);
        let mut response_builder = Response::builder().status(parts.status);
        for (name, value) in &parts.headers {
            response_builder = response_builder.header(name, value);
        }
        response_builder.body(body).map_err(|error| {
            Error::Server(format!("failed to build upstream http3 response: {error}"))
        })
    }

    async fn session_for(
        &self,
        key: Http3SessionKey,
        peer_url: &str,
    ) -> Result<Arc<Http3Session>, Error> {
        loop {
            enum SessionAction {
                Wait(Arc<Notify>),
                Connect(Arc<Notify>),
            }

            let action = {
                let mut sessions = self.sessions.lock().await;
                match sessions.get(&key).cloned() {
                    Some(Http3SessionEntry::Ready(existing)) if !existing.is_closed() => {
                        return Ok(existing);
                    }
                    Some(Http3SessionEntry::Pending(notify)) => SessionAction::Wait(notify),
                    Some(Http3SessionEntry::Ready(_)) | None => {
                        let notify = Arc::new(Notify::new());
                        sessions.insert(key.clone(), Http3SessionEntry::Pending(notify.clone()));
                        SessionAction::Connect(notify)
                    }
                }
            };

            match action {
                SessionAction::Wait(notify) => notify.notified().await,
                SessionAction::Connect(notify) => {
                    let result = self.connect_session(&key, peer_url).await.map(Arc::new);
                    let mut sessions = self.sessions.lock().await;
                    match &result {
                        Ok(session) => {
                            sessions.insert(key.clone(), Http3SessionEntry::Ready(session.clone()));
                        }
                        Err(_) => {
                            sessions.remove(&key);
                        }
                    }
                    notify.notify_waiters();
                    return result;
                }
            }
        }
    }

    async fn connect_session(
        &self,
        key: &Http3SessionKey,
        peer_url: &str,
    ) -> Result<Http3Session, Error> {
        let endpoint = self.endpoint_for_remote(key.remote_addr).await?;
        let connecting = endpoint.connect(key.remote_addr, &key.server_name).map_err(|error| {
            Error::Server(format!(
                "failed to start upstream http3 connect to `{}`: {error}",
                peer_url
            ))
        })?;
        let connection = tokio::time::timeout(self.connect_timeout, connecting)
            .await
            .map_err(|_| {
                Error::Server(format!(
                    "upstream http3 connect to `{}` timed out after {} ms",
                    peer_url,
                    self.connect_timeout.as_millis()
                ))
            })?
            .map_err(|error| {
                Error::Server(format!("upstream http3 connect to `{}` failed: {error}", peer_url))
            })?;

        let (mut driver, send_request) =
            client::new(h3_quinn::Connection::new(connection)).await.map_err(|error| {
                Error::Server(format!(
                    "failed to initialize upstream http3 session for `{}`: {error}",
                    peer_url
                ))
            })?;
        let session = Http3Session::new(send_request);
        let closed = session.closed.clone();
        let driver_closed = closed.clone();
        let driver_task = tokio::spawn(async move {
            let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
            driver_closed.store(true, Ordering::Release);
        });
        let session = {
            let session = session;
            session.set_driver_task(driver_task).await;
            session
        };
        Ok(session)
    }

    async fn endpoint_for_remote(&self, remote_addr: SocketAddr) -> Result<quinn::Endpoint, Error> {
        let cache = match remote_addr {
            SocketAddr::V4(_) => &self.endpoints.ipv4,
            SocketAddr::V6(_) => &self.endpoints.ipv6,
        };
        let mut endpoint = cache.lock().await;
        if let Some(endpoint) = endpoint.as_ref() {
            return Ok(endpoint.clone());
        }

        let bind_addr = match remote_addr {
            SocketAddr::V4(_) => "0.0.0.0:0".parse().unwrap(),
            SocketAddr::V6(_) => "[::]:0".parse().unwrap(),
        };
        let mut created = quinn::Endpoint::client(bind_addr).map_err(Error::Io)?;
        created.set_default_client_config(self.client_config.clone());
        let reusable = created.clone();
        *endpoint = Some(created);
        Ok(reusable)
    }

    #[cfg(test)]
    async fn cached_endpoint_count(&self) -> usize {
        usize::from(self.endpoints.ipv4.lock().await.is_some())
            + usize::from(self.endpoints.ipv6.lock().await.is_some())
    }

    #[cfg(test)]
    async fn cached_endpoint_local_addr(
        &self,
        remote_addr: SocketAddr,
    ) -> Result<SocketAddr, Error> {
        self.endpoint_for_remote(remote_addr).await?.local_addr().map_err(Error::Io)
    }

    #[cfg(test)]
    async fn cached_session_count(&self) -> usize {
        self.sessions
            .lock()
            .await
            .values()
            .filter(|entry| matches!(entry, Http3SessionEntry::Ready(_)))
            .count()
    }
}

struct StreamingResponseBody {
    rx: mpsc::Receiver<Result<Frame<Bytes>, BoxError>>,
    size_hint: SizeHint,
    done: bool,
    join_handle: Option<JoinHandle<()>>,
}

impl StreamingResponseBody {
    fn new(
        rx: mpsc::Receiver<Result<Frame<Bytes>, BoxError>>,
        size_hint: SizeHint,
        join_handle: JoinHandle<()>,
    ) -> Self {
        Self { rx, size_hint, done: false, join_handle: Some(join_handle) }
    }
}

impl Drop for StreamingResponseBody {
    fn drop(&mut self) {
        if let Some(join_handle) = self.join_handle.take()
            && !join_handle.is_finished()
        {
            join_handle.abort();
        }
    }
}

impl hyper::body::Body for StreamingResponseBody {
    type Data = Bytes;
    type Error = BoxError;

    fn poll_frame(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.as_mut().get_mut();
        match this.rx.poll_recv(cx) {
            std::task::Poll::Ready(None) => {
                this.done = true;
                std::task::Poll::Ready(None)
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done
    }

    fn size_hint(&self) -> SizeHint {
        self.size_hint.clone()
    }
}

/// Streams an upstream HTTP/3 response body into a Hyper body adapter.
fn streaming_response_body(
    mut request_stream: H3RequestStream,
    session: Arc<Http3Session>,
    peer_url: String,
    size_hint: SizeHint,
) -> HttpBody {
    let (tx, rx) = mpsc::channel(1);
    let join_handle = tokio::spawn(async move {
        loop {
            let next = match request_stream.recv_data().await {
                Ok(chunk) => chunk,
                Err(error) if is_clean_http3_response_shutdown(&error) => None,
                Err(error) => {
                    session.mark_closed();
                    let _ = tx
                        .send(Err::<Frame<Bytes>, BoxError>(
                            std::io::Error::other(format!(
                                "failed to receive upstream http3 response body from `{peer_url}`: {error}"
                            ))
                            .into(),
                        ))
                        .await;
                    return;
                }
            };
            let Some(mut chunk) = next else {
                break;
            };
            let bytes = chunk.copy_to_bytes(chunk.remaining());
            if tx.send(Ok(Frame::data(bytes))).await.is_err() {
                return;
            }
        }

        match request_stream.recv_trailers().await {
            Ok(Some(trailers)) => {
                let _ = tx.send(Ok(Frame::trailers(trailers))).await;
            }
            Ok(None) => {}
            Err(error) if is_clean_http3_response_shutdown(&error) => {}
            Err(error) => {
                session.mark_closed();
                let _ = tx
                    .send(Err::<Frame<Bytes>, BoxError>(
                        std::io::Error::other(format!(
                            "failed to receive upstream http3 response trailers from `{peer_url}`: {error}"
                        ))
                        .into(),
                    ))
                    .await;
            }
        }
    });

    boxed_body(StreamingResponseBody::new(rx, size_hint, join_handle))
}

/// Derives a Hyper body size hint from upstream response headers.
fn response_size_hint(headers: &HeaderMap) -> SizeHint {
    let mut hint = SizeHint::default();
    if let Some(content_length) = headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        hint.set_exact(content_length);
    }
    hint
}

/// Detects peer shutdown errors that should be treated as a clean HTTP/3 EOF.
fn is_clean_http3_response_shutdown(error: &impl std::fmt::Display) -> bool {
    let error = error.to_string();
    error.contains("ApplicationClose: H3_NO_ERROR")
        || error.contains("Application { code: H3_NO_ERROR")
}

/// Resolves the selected upstream peer authority into a socket address.
fn resolve_peer_socket_addr(peer: &UpstreamPeer) -> Result<SocketAddr, Error> {
    peer.authority
        .to_socket_addrs()
        .map_err(|error| {
            Error::Server(format!(
                "failed to resolve upstream http3 peer authority `{}`: {error}",
                peer.authority
            ))
        })?
        .next()
        .ok_or_else(|| {
            Error::Server(format!(
                "upstream http3 peer authority `{}` did not resolve to any address",
                peer.authority
            ))
        })
}

/// Determines the TLS server name to use for an upstream HTTP/3 connection.
fn server_name_for_peer(upstream: &Upstream, peer: &UpstreamPeer) -> Result<String, Error> {
    if let Some(server_name_override) = upstream.server_name_override.as_ref() {
        return Ok(server_name_override.clone());
    }

    peer.url.parse::<http::Uri>().ok().and_then(|uri| uri.host().map(str::to_string)).ok_or_else(
        || {
            Error::Server(format!(
                "failed to derive TLS server name for upstream http3 peer `{}`",
                peer.url
            ))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::StatusCode;
    use http_body_util::BodyExt;
    use rginx_core::{UpstreamLoadBalance, UpstreamSettings, UpstreamTls};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    #[tokio::test]
    async fn reuses_cached_endpoint_for_repeated_ipv4_requests() {
        let client_config = super::super::tls::build_http3_client_config(
            &UpstreamTls::Insecure,
            None,
            None,
            None,
            None,
            true,
        )
        .expect("http3 client config should build");
        let client = Http3Client::new(client_config, Duration::from_secs(1));
        let remote_addr: SocketAddr = "127.0.0.1:443".parse().unwrap();

        assert_eq!(client.cached_endpoint_count().await, 0);

        let first = client
            .cached_endpoint_local_addr(remote_addr)
            .await
            .expect("first endpoint should be created");
        assert_eq!(client.cached_endpoint_count().await, 1);

        let second = client
            .cached_endpoint_local_addr(remote_addr)
            .await
            .expect("cached endpoint should be reused");
        assert_eq!(first, second);
        assert_eq!(client.cached_endpoint_count().await, 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reuses_http3_session_for_sequential_requests() {
        let client_config = super::super::tls::build_http3_client_config(
            &UpstreamTls::Insecure,
            None,
            None,
            None,
            None,
            true,
        )
        .expect("http3 client config should build");
        let client = Http3Client::new(client_config, Duration::from_secs(1));
        let accepted_connections = Arc::new(AtomicUsize::new(0));
        let request_count = Arc::new(AtomicUsize::new(0));
        let (listen_addr, server_task) =
            spawn_test_http3_server(accepted_connections.clone(), request_count.clone(), None);

        let upstream = Arc::new(Upstream::new(
            "backend".to_string(),
            vec![UpstreamPeer {
                url: format!("https://127.0.0.1:{}", listen_addr.port()),
                scheme: "https".to_string(),
                authority: format!("127.0.0.1:{}", listen_addr.port()),
                weight: 1,
                backup: false,
            }],
            UpstreamTls::Insecure,
            UpstreamSettings {
                protocol: UpstreamProtocol::Http3,
                load_balance: UpstreamLoadBalance::RoundRobin,
                server_name: true,
                server_name_override: Some("localhost".to_string()),
                tls_versions: None,
                server_verify_depth: None,
                server_crl_path: None,
                client_identity: None,
                request_timeout: Duration::from_secs(5),
                connect_timeout: Duration::from_secs(5),
                write_timeout: Duration::from_secs(5),
                idle_timeout: Duration::from_secs(5),
                pool_idle_timeout: None,
                pool_max_idle_per_host: usize::MAX,
                tcp_keepalive: None,
                tcp_nodelay: false,
                http2_keep_alive_interval: None,
                http2_keep_alive_timeout: Duration::from_secs(20),
                http2_keep_alive_while_idle: false,
                max_replayable_request_body_bytes: 64 * 1024,
                unhealthy_after_failures: 2,
                unhealthy_cooldown: Duration::from_secs(10),
                active_health_check: None,
            },
        ));
        let peer = upstream.peers[0].clone();

        let first = client
            .request(upstream.as_ref(), &peer, test_request(&peer.url, "/first"))
            .await
            .expect("first request should succeed");
        let first_body =
            first.into_body().collect().await.expect("first body should collect").to_bytes();
        assert_eq!(first_body, Bytes::from_static(b"ok\n"));

        let second = client
            .request(upstream.as_ref(), &peer, test_request(&peer.url, "/second"))
            .await
            .expect("second request should succeed");
        let second_body =
            second.into_body().collect().await.expect("second body should collect").to_bytes();
        assert_eq!(second_body, Bytes::from_static(b"ok\n"));

        assert_eq!(accepted_connections.load(Ordering::Relaxed), 1);
        assert_eq!(request_count.load(Ordering::Relaxed), 2);
        assert_eq!(client.cached_session_count().await, 1);

        server_task.abort();
        let _ = server_task.await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn streams_http3_response_body_without_full_buffering() {
        let client_config = super::super::tls::build_http3_client_config(
            &UpstreamTls::Insecure,
            None,
            None,
            None,
            None,
            true,
        )
        .expect("http3 client config should build");
        let client = Http3Client::new(client_config, Duration::from_secs(1));
        let delay = Duration::from_millis(300);
        let (listen_addr, server_task) = spawn_test_http3_server(
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
            Some(delay),
        );

        let upstream = Arc::new(Upstream::new(
            "backend".to_string(),
            vec![UpstreamPeer {
                url: format!("https://127.0.0.1:{}", listen_addr.port()),
                scheme: "https".to_string(),
                authority: format!("127.0.0.1:{}", listen_addr.port()),
                weight: 1,
                backup: false,
            }],
            UpstreamTls::Insecure,
            UpstreamSettings {
                protocol: UpstreamProtocol::Http3,
                load_balance: UpstreamLoadBalance::RoundRobin,
                server_name: true,
                server_name_override: Some("localhost".to_string()),
                tls_versions: None,
                server_verify_depth: None,
                server_crl_path: None,
                client_identity: None,
                request_timeout: Duration::from_secs(5),
                connect_timeout: Duration::from_secs(5),
                write_timeout: Duration::from_secs(5),
                idle_timeout: Duration::from_secs(5),
                pool_idle_timeout: None,
                pool_max_idle_per_host: usize::MAX,
                tcp_keepalive: None,
                tcp_nodelay: false,
                http2_keep_alive_interval: None,
                http2_keep_alive_timeout: Duration::from_secs(20),
                http2_keep_alive_while_idle: false,
                max_replayable_request_body_bytes: 64 * 1024,
                unhealthy_after_failures: 2,
                unhealthy_cooldown: Duration::from_secs(10),
                active_health_check: None,
            },
        ));
        let peer = upstream.peers[0].clone();

        let started = Instant::now();
        let response = client
            .request(upstream.as_ref(), &peer, test_request(&peer.url, "/stream"))
            .await
            .expect("streaming request should succeed");
        assert!(started.elapsed() < delay);

        let mut body = response.into_body();
        let first = body
            .frame()
            .await
            .expect("first frame should exist")
            .expect("first frame should be ok")
            .into_data()
            .expect("first frame should contain data");
        assert_eq!(first, Bytes::from_static(b"part-1\n"));

        let second = body
            .frame()
            .await
            .expect("second frame should exist")
            .expect("second frame should be ok")
            .into_data()
            .expect("second frame should contain data");
        assert_eq!(second, Bytes::from_static(b"part-2\n"));

        server_task.abort();
        let _ = server_task.await;
    }

    fn test_request(base_url: &str, path: &str) -> Request<HttpBody> {
        Request::builder()
            .method("GET")
            .uri(format!("{base_url}{path}"))
            .body(boxed_body(http_body_util::Empty::<Bytes>::new()))
            .expect("request should build")
    }

    fn spawn_test_http3_server(
        accepted_connections: Arc<AtomicUsize>,
        request_count: Arc<AtomicUsize>,
        delayed_second_chunk: Option<Duration>,
    ) -> (SocketAddr, JoinHandle<()>) {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("cert should generate");
        let cert_der = rustls::pki_types::CertificateDer::from(cert.cert);
        let key_der = rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
        let mut server_crypto = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der], key_der.into())
            .expect("server cert should configure");
        server_crypto.alpn_protocols = vec![b"h3".to_vec()];
        let server_config = quinn::ServerConfig::with_crypto(Arc::new(
            quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)
                .expect("quic server config should build"),
        ));
        let endpoint =
            quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap()).unwrap();
        let listen_addr = endpoint.local_addr().expect("listen addr should exist");

        let task = tokio::spawn(async move {
            let incoming = endpoint.accept().await.expect("connection should arrive");
            accepted_connections.fetch_add(1, Ordering::Relaxed);
            let connection = incoming.await.expect("connection should establish");
            let mut h3 = h3::server::Connection::new(h3_quinn::Connection::new(connection))
                .await
                .expect("h3 server should initialize");

            while request_count.load(Ordering::Relaxed) < 2 || delayed_second_chunk.is_some() {
                let Some(resolver) = h3.accept().await.expect("h3 accept should succeed") else {
                    break;
                };
                let (_request, mut stream) =
                    resolver.resolve_request().await.expect("request should resolve");
                request_count.fetch_add(1, Ordering::Relaxed);
                stream
                    .send_response(Response::builder().status(StatusCode::OK).body(()).unwrap())
                    .await
                    .expect("response headers should send");
                if let Some(delay) = delayed_second_chunk {
                    stream
                        .send_data(Bytes::from_static(b"part-1\n"))
                        .await
                        .expect("first chunk should send");
                    tokio::time::sleep(delay).await;
                    stream
                        .send_data(Bytes::from_static(b"part-2\n"))
                        .await
                        .expect("second chunk should send");
                    stream.finish().await.expect("stream should finish");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    break;
                } else {
                    stream.send_data(Bytes::from_static(b"ok\n")).await.expect("body should send");
                    stream.finish().await.expect("stream should finish");
                    if request_count.load(Ordering::Relaxed) >= 2 {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        break;
                    }
                }
            }
        });

        (listen_addr, task)
    }
}
