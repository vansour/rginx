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

#[tokio::test(flavor = "multi_thread")]
async fn proxies_basic_grpc_over_http3_to_http3_upstreams_with_response_trailers() {
    let cert = generate_cert("localhost");
    let shared_dir = temp_dir("rginx-grpc-http3-shared");
    fs::create_dir_all(&shared_dir).expect("shared temp dir should be created");
    let server_cert_path = shared_dir.join("server.crt");
    let server_key_path = shared_dir.join("server.key");
    fs::write(&server_cert_path, cert.cert.pem()).expect("server cert should be written");
    fs::write(&server_key_path, cert.signing_key.serialize_pem())
        .expect("server key should be written");

    let (upstream_addr, observed_rx, upstream_task, upstream_temp_dir) =
        spawn_h3_grpc_upstream(&server_cert_path, &server_key_path, UpstreamMode::Immediate).await;

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-grpc-http3-upstream",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| {
            grpc_http3_proxy_config(listen_addr, upstream_addr, cert_path, key_path)
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let response = h3_request(
        listen_addr,
        "localhost",
        "POST",
        GRPC_METHOD_PATH,
        &[(CONTENT_TYPE.as_str(), "application/grpc"), (TE.as_str(), "trailers")],
        Some(Bytes::from_static(GRPC_REQUEST_FRAME)),
        &cert.cert.pem(),
    )
    .await
    .expect("grpc over http3 request should succeed");

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.headers.get(CONTENT_TYPE.as_str()).map(String::as_str),
        Some("application/grpc")
    );
    assert_eq!(
        response.body,
        Bytes::from_static(GRPC_RESPONSE_FRAME),
        "response headers={:?} trailers={:?}\nlogs:\n{}",
        response.headers,
        response.trailers,
        server.combined_output()
    );
    assert_eq!(
        response
            .trailers
            .as_ref()
            .and_then(|trailers| trailers.get("grpc-status"))
            .and_then(|value| value.to_str().ok()),
        Some("0")
    );
    assert_eq!(
        response
            .trailers
            .as_ref()
            .and_then(|trailers| trailers.get("grpc-message"))
            .and_then(|value| value.to_str().ok()),
        Some("ok")
    );

    let observed = tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .expect("upstream request should be observed before timeout")
        .expect("upstream observation channel should complete");
    assert_eq!(observed.path, GRPC_METHOD_PATH);
    assert_eq!(observed.content_type.as_deref(), Some("application/grpc"));

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream h3 grpc task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
    fs::remove_dir_all(shared_dir).expect("shared temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn proxies_grpc_web_binary_over_http3_to_http3_upstreams() {
    let cert = generate_cert("localhost");
    let shared_dir = temp_dir("rginx-grpc-web-http3-shared");
    fs::create_dir_all(&shared_dir).expect("shared temp dir should be created");
    let server_cert_path = shared_dir.join("server.crt");
    let server_key_path = shared_dir.join("server.key");
    fs::write(&server_cert_path, cert.cert.pem()).expect("server cert should be written");
    fs::write(&server_key_path, cert.signing_key.serialize_pem())
        .expect("server key should be written");

    let (upstream_addr, _observed_rx, upstream_task, upstream_temp_dir) =
        spawn_h3_grpc_upstream(&server_cert_path, &server_key_path, UpstreamMode::Immediate).await;

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-grpc-web-http3-upstream",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| {
            grpc_http3_proxy_config(listen_addr, upstream_addr, cert_path, key_path)
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let response = h3_request(
        listen_addr,
        "localhost",
        "POST",
        GRPC_METHOD_PATH,
        &[(CONTENT_TYPE.as_str(), "application/grpc-web+proto"), ("x-grpc-web", "1")],
        Some(Bytes::from_static(GRPC_REQUEST_FRAME)),
        &cert.cert.pem(),
    )
    .await
    .expect("grpc-web over http3 request should succeed");

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(
        response.headers.get(CONTENT_TYPE.as_str()).map(String::as_str),
        Some("application/grpc-web+proto")
    );
    let (frames, trailers) = decode_grpc_web_response(response.body.as_ref());
    assert_eq!(frames, vec![Bytes::copy_from_slice(&GRPC_RESPONSE_FRAME[5..])]);
    assert_eq!(trailers.get("grpc-status").and_then(|value| value.to_str().ok()), Some("0"));
    assert_eq!(trailers.get("grpc-message").and_then(|value| value.to_str().ok()), Some("ok"));

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream h3 grpc task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
    fs::remove_dir_all(shared_dir).expect("shared temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn grpc_timeout_over_http3_upstream_returns_deadline_exceeded() {
    let cert = generate_cert("localhost");
    let shared_dir = temp_dir("rginx-grpc-http3-timeout-shared");
    fs::create_dir_all(&shared_dir).expect("shared temp dir should be created");
    let server_cert_path = shared_dir.join("server.crt");
    let server_key_path = shared_dir.join("server.key");
    fs::write(&server_cert_path, cert.cert.pem()).expect("server cert should be written");
    fs::write(&server_key_path, cert.signing_key.serialize_pem())
        .expect("server key should be written");

    let (upstream_addr, _observed_rx, upstream_task, upstream_temp_dir) = spawn_h3_grpc_upstream(
        &server_cert_path,
        &server_key_path,
        UpstreamMode::DelayHeaders(Duration::from_secs(2)),
    )
    .await;

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-grpc-http3-timeout",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| {
            grpc_http3_proxy_config_with_timeout(listen_addr, upstream_addr, cert_path, key_path, 1)
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let response = h3_request(
        listen_addr,
        "localhost",
        "POST",
        GRPC_METHOD_PATH,
        &[(CONTENT_TYPE.as_str(), "application/grpc"), (TE.as_str(), "trailers")],
        Some(Bytes::from_static(GRPC_REQUEST_FRAME)),
        &cert.cert.pem(),
    )
    .await
    .expect("grpc timeout request should succeed");

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.headers.get("grpc-status").map(String::as_str), Some("4"));

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.abort();
    let _ = upstream_task.await;
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
    fs::remove_dir_all(shared_dir).expect("shared temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn active_grpc_health_checks_can_target_http3_upstreams() {
    let cert = generate_cert("localhost");
    let shared_dir = temp_dir("rginx-grpc-http3-health-shared");
    fs::create_dir_all(&shared_dir).expect("shared temp dir should be created");
    let server_cert_path = shared_dir.join("server.crt");
    let server_key_path = shared_dir.join("server.key");
    fs::write(&server_cert_path, cert.cert.pem()).expect("server cert should be written");
    fs::write(&server_key_path, cert.signing_key.serialize_pem())
        .expect("server key should be written");

    let (upstream_addr, health_seen_rx, upstream_task, upstream_temp_dir) =
        spawn_h3_grpc_health_upstream(&server_cert_path, &server_key_path).await;

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-grpc-http3-health", |_| {
        grpc_http3_health_config(listen_addr, upstream_addr)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    tokio::time::timeout(Duration::from_secs(5), health_seen_rx)
        .await
        .expect("health probe should arrive before timeout")
        .expect("health probe channel should complete");

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.abort();
    let _ = upstream_task.await;
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
    fs::remove_dir_all(shared_dir).expect("shared temp dir should be removed");
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedGrpcRequest {
    path: String,
    content_type: Option<String>,
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
    let driver_task = tokio::spawn(async move {
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

    driver_task.abort();
    endpoint.close(quinn::VarInt::from_u32(0), b"done");

    Ok(H3Response { status, headers, body: response_body.freeze(), trailers })
}

async fn spawn_h3_grpc_upstream(
    cert_path: &Path,
    key_path: &Path,
    mode: UpstreamMode,
) -> (SocketAddr, oneshot::Receiver<ObservedGrpcRequest>, JoinHandle<()>, PathBuf) {
    let temp_dir = temp_dir("rginx-grpc-http3-upstream");
    fs::create_dir_all(&temp_dir).expect("upstream temp dir should be created");

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
) -> (SocketAddr, oneshot::Receiver<()>, JoinHandle<()>, PathBuf) {
    let temp_dir = temp_dir("rginx-grpc-http3-health-upstream");
    fs::create_dir_all(&temp_dir).expect("upstream temp dir should be created");

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

fn grpc_http3_proxy_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    cert_path: &Path,
    key_path: &Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [UpstreamPeerConfig(url: {:?})],\n            tls: Some(Insecure),\n            protocol: Http3,\n            server_name_override: Some(\"localhost\"),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/grpc.health.v1.Health\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn grpc_http3_proxy_config_with_timeout(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    cert_path: &Path,
    key_path: &Path,
    timeout_secs: u64,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [UpstreamPeerConfig(url: {:?})],\n            tls: Some(Insecure),\n            protocol: Http3,\n            request_timeout_secs: Some({timeout_secs}),\n            server_name_override: Some(\"localhost\"),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/grpc.health.v1.Health\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        timeout_secs = timeout_secs,
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn grpc_http3_health_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [UpstreamPeerConfig(url: {:?})],\n            tls: Some(Insecure),\n            protocol: Http3,\n            server_name_override: Some(\"localhost\"),\n            health_check_grpc_service: Some(\"grpc.health.v1.Health\"),\n            health_check_interval_secs: Some(1),\n            health_check_timeout_secs: Some(1),\n            healthy_successes_required: Some(1),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn root_store_from_pem(cert_pem: &str) -> Result<RootCertStore, String> {
    let cert = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to parse certificate PEM: {error}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "certificate PEM did not contain a certificate".to_string())?;
    let mut roots = RootCertStore::empty();
    roots.add(cert).map_err(|error| format!("failed to add root certificate: {error}"))?;
    Ok(roots)
}

fn load_certs(path: &Path) -> Vec<CertificateDer<'static>> {
    CertificateDer::pem_file_iter(path)
        .expect("certificate file should open")
        .collect::<Result<Vec<_>, _>>()
        .expect("certificate PEM should parse")
}

fn load_private_key(path: &Path) -> PrivateKeyDer<'static> {
    PrivateKeyDer::from_pem_file(path).expect("private key PEM should parse")
}

fn temp_dir(prefix: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!("{prefix}-{unique}-{id}"))
}

fn generate_cert(hostname: &str) -> rcgen::CertifiedKey<rcgen::KeyPair> {
    rcgen::generate_simple_self_signed(vec![hostname.to_string()])
        .expect("self-signed certificate should generate")
}

fn decode_grpc_web_response(body: &[u8]) -> (Vec<Bytes>, HeaderMap) {
    let mut frames = Vec::new();
    let mut trailers = HeaderMap::new();
    let mut cursor = body;

    while cursor.len() >= 5 {
        let flags = cursor[0];
        let len = u32::from_be_bytes([cursor[1], cursor[2], cursor[3], cursor[4]]) as usize;
        let frame_len = 5 + len;
        let frame = &cursor[..frame_len];
        let payload = &frame[5..];
        if flags & 0x80 == 0 {
            frames.push(Bytes::copy_from_slice(payload));
        } else {
            for line in payload.split(|byte| *byte == b'\n') {
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                if line.is_empty() {
                    continue;
                }
                let separator = line
                    .iter()
                    .position(|byte| *byte == b':')
                    .expect("trailer line should contain ':'");
                let (name, value) = line.split_at(separator);
                trailers.append(
                    hyper::http::header::HeaderName::from_bytes(name)
                        .expect("trailer name should parse"),
                    HeaderValue::from_bytes(value[1..].trim_ascii())
                        .expect("trailer value should parse"),
                );
            }
        }
        cursor = &cursor[frame_len..];
    }

    (frames, trailers)
}
