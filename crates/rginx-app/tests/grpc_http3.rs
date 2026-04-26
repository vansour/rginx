use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::{Buf, Bytes, BytesMut};
use h3::client;
use h3::server::Connection as H3Connection;
use h3_quinn::quinn;
use hyper::http::HeaderMap;
use hyper::http::header::{CONTENT_TYPE, HeaderValue, TE};
use hyper::http::{Request, Response, StatusCode};
use quinn::crypto::rustls::{QuicClientConfig, QuicServerConfig};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

const GRPC_METHOD_PATH: &str = "/grpc.health.v1.Health/Check";
const GRPC_REQUEST_FRAME: &[u8] = b"\x00\x00\x00\x00\x02hi";
const GRPC_RESPONSE_FRAME: &[u8] = b"\x00\x00\x00\x00\x02ok";

#[path = "grpc_http3/config.rs"]
mod config;
#[path = "grpc_http3/health.rs"]
mod health;
#[path = "grpc_http3/helpers.rs"]
mod helpers;
#[path = "grpc_http3/proxy.rs"]
mod proxy;
#[path = "grpc_http3/timeout.rs"]
mod timeout;

use config::*;
use helpers::*;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedGrpcRequest {
    path: String,
    content_type: Option<String>,
}

#[derive(Debug)]
struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new(prefix: &str) -> Self {
        let path = temp_dir(prefix);
        fs::create_dir_all(&path).expect("shared temp dir should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[derive(Debug)]
struct H3Response {
    status: StatusCode,
    headers: std::collections::HashMap<String, String>,
    body: Bytes,
    trailers: Option<HeaderMap>,
}

#[derive(Clone, Copy)]
enum UpstreamMode {
    Immediate,
    DelayHeaders(Duration),
}

async fn h3_request(
    listen_addr: SocketAddr,
    server_name: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Bytes>,
    cert_pem: &str,
) -> Result<H3Response, String> {
    let roots = root_store_from_pem(cert_pem)?;
    let mut client_crypto = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|error| format!("failed to constrain TLS versions for http3 client: {error}"))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    client_crypto.alpn_protocols = vec![b"h3".to_vec()];

    let client_config = quinn::ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(client_crypto)
            .map_err(|error| format!("failed to build quic client config: {error}"))?,
    ));
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap())
        .map_err(|error| error.to_string())?;
    endpoint.set_default_client_config(client_config);

    let connection = endpoint
        .connect(listen_addr, server_name)
        .map_err(|error| format!("failed to start quic connect: {error}"))?
        .await
        .map_err(|error| format!("quic connect failed: {error}"))?;

    let (mut driver, mut send_request) =
        client::new(h3_quinn::Connection::new(connection))
            .await
            .map_err(|error| format!("failed to initialize http3 client: {error}"))?;
    let mut driver_task = tokio::spawn(async move {
        let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });

    let mut request_builder = Request::builder()
        .method(method)
        .uri(format!("https://{server_name}:{}{path}", listen_addr.port()));
    for (name, value) in headers {
        request_builder = request_builder.header(*name, *value);
    }
    let mut request_stream = send_request
        .send_request(request_builder.body(()).expect("http3 request should build"))
        .await
        .map_err(|error| format!("failed to send http3 request: {error}"))?;
    if let Some(body) = body {
        request_stream
            .send_data(body)
            .await
            .map_err(|error| format!("failed to send http3 request body: {error}"))?;
    }
    request_stream
        .finish()
        .await
        .map_err(|error| format!("failed to finish http3 request: {error}"))?;

    let response = request_stream
        .recv_response()
        .await
        .map_err(|error| format!("failed to receive http3 response headers: {error}"))?;
    let status = response.status();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value.to_str().ok().map(|value| (name.as_str().to_ascii_lowercase(), value.to_string()))
        })
        .collect::<std::collections::HashMap<_, _>>();
    let mut response_body = BytesMut::new();
    while let Some(mut chunk) = request_stream
        .recv_data()
        .await
        .map_err(|error| format!("failed to receive http3 response body: {error}"))?
    {
        response_body.extend_from_slice(chunk.copy_to_bytes(chunk.remaining()).as_ref());
    }
    let trailers = request_stream
        .recv_trailers()
        .await
        .map_err(|error| format!("failed to receive http3 response trailers: {error}"))?;

    if tokio::time::timeout(Duration::from_millis(50), &mut driver_task).await.is_err() {
        driver_task.abort();
        let _ = driver_task.await;
    }
    endpoint.close(quinn::VarInt::from_u32(0), b"done");

    Ok(H3Response { status, headers, body: response_body.freeze(), trailers })
}

async fn spawn_h3_grpc_upstream(
    cert_path: &Path,
    key_path: &Path,
    mode: UpstreamMode,
) -> (SocketAddr, oneshot::Receiver<ObservedGrpcRequest>, JoinHandle<()>, TempDirGuard) {
    let temp_dir = TempDirGuard::new("rginx-grpc-http3-upstream");

    let mut server_crypto = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .expect("server TLS1.3 builder should succeed")
    .with_no_client_auth()
    .with_single_cert(load_certs(cert_path), load_private_key(key_path))
    .expect("test upstream TLS config should build");
    server_crypto.alpn_protocols = vec![b"h3".to_vec()];
    let server_config = quinn::ServerConfig::with_crypto(Arc::new(
        QuicServerConfig::try_from(server_crypto).expect("quic server config should build"),
    ));
    let endpoint = quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap()).unwrap();
    let listen_addr = endpoint.local_addr().expect("upstream h3 addr should exist");
    let (observed_tx, observed_rx) = oneshot::channel();

    let task = tokio::spawn(async move {
        let incoming = endpoint.accept().await.expect("upstream h3 connection should arrive");
        let connection = incoming.await.expect("upstream h3 connection should establish");
        let mut h3 = H3Connection::new(h3_quinn::Connection::new(connection))
            .await
            .expect("upstream h3 should initialize");
        let resolver = h3
            .accept()
            .await
            .expect("upstream h3 should accept request")
            .expect("request should exist");
        let (request, mut stream) =
            resolver.resolve_request().await.expect("upstream h3 should resolve request");
        let _ = observed_tx.send(ObservedGrpcRequest {
            path: request.uri().path().to_string(),
            content_type: request
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string),
        });
        while let Some(mut chunk) =
            stream.recv_data().await.expect("upstream h3 should read request body")
        {
            let _ = chunk.copy_to_bytes(chunk.remaining());
        }
        let _ = stream.recv_trailers().await.expect("upstream h3 should read request trailers");

        if let UpstreamMode::DelayHeaders(delay) = mode {
            tokio::time::sleep(delay).await;
        }

        stream
            .send_response(
                Response::builder()
                    .status(StatusCode::OK)
                    .header(CONTENT_TYPE, "application/grpc")
                    .body(())
                    .expect("response should build"),
            )
            .await
            .expect("upstream h3 should send response");
        stream
            .send_data(Bytes::from_static(GRPC_RESPONSE_FRAME))
            .await
            .expect("upstream h3 should send body");
        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", HeaderValue::from_static("0"));
        trailers.insert("grpc-message", HeaderValue::from_static("ok"));
        stream.send_trailers(trailers).await.expect("upstream h3 should send trailers");
        stream.finish().await.expect("upstream h3 should finish response");
        tokio::time::sleep(Duration::from_millis(50)).await;
    });

    (listen_addr, observed_rx, task, temp_dir)
}

async fn spawn_h3_grpc_health_upstream(
    cert_path: &Path,
    key_path: &Path,
) -> (SocketAddr, oneshot::Receiver<()>, JoinHandle<()>, TempDirGuard) {
    let temp_dir = TempDirGuard::new("rginx-grpc-http3-health-upstream");

    let mut server_crypto = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .expect("server TLS1.3 builder should succeed")
    .with_no_client_auth()
    .with_single_cert(load_certs(cert_path), load_private_key(key_path))
    .expect("test upstream TLS config should build");
    server_crypto.alpn_protocols = vec![b"h3".to_vec()];
    let server_config = quinn::ServerConfig::with_crypto(Arc::new(
        QuicServerConfig::try_from(server_crypto).expect("quic server config should build"),
    ));
    let endpoint = quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap()).unwrap();
    let listen_addr = endpoint.local_addr().expect("upstream h3 addr should exist");
    let (health_seen_tx, health_seen_rx) = oneshot::channel();
    let health_seen_tx = Arc::new(Mutex::new(Some(health_seen_tx)));

    let task = tokio::spawn(async move {
        let incoming = endpoint.accept().await.expect("upstream h3 connection should arrive");
        let connection = incoming.await.expect("upstream h3 connection should establish");
        let mut h3 = H3Connection::new(h3_quinn::Connection::new(connection))
            .await
            .expect("upstream h3 should initialize");
        let resolver = h3
            .accept()
            .await
            .expect("upstream h3 should accept request")
            .expect("request should exist");
        let (request, mut stream) =
            resolver.resolve_request().await.expect("upstream h3 should resolve request");
        assert_eq!(request.uri().path(), "/grpc.health.v1.Health/Check");
        while let Some(mut chunk) =
            stream.recv_data().await.expect("upstream h3 should read health body")
        {
            let _ = chunk.copy_to_bytes(chunk.remaining());
        }
        let _ = stream.recv_trailers().await.expect("upstream h3 should read health trailers");
        if let Some(sender) =
            health_seen_tx.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()
        {
            let _ = sender.send(());
        }

        let mut body = BytesMut::new();
        body.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x02]);
        body.extend_from_slice(b"\x08\x01");
        stream
            .send_response(
                Response::builder()
                    .status(StatusCode::OK)
                    .header(CONTENT_TYPE, "application/grpc")
                    .body(())
                    .expect("response should build"),
            )
            .await
            .expect("upstream h3 should send response");
        stream
            .send_data(body.freeze())
            .await
            .expect("upstream h3 should send health response body");
        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", HeaderValue::from_static("0"));
        stream.send_trailers(trailers).await.expect("upstream h3 should send health trailers");
        stream.finish().await.expect("upstream h3 should finish response");
        tokio::time::sleep(Duration::from_millis(50)).await;
    });

    (listen_addr, health_seen_rx, task, temp_dir)
}
