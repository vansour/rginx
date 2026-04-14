use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::{Buf, Bytes, BytesMut};
use flate2::read::GzDecoder;
use h3::client;
use http_body_util::Empty;
use hyper::http::{Request, StatusCode};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use quinn::crypto::rustls::QuicClientConfig;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
};
use rustls::pki_types::{CertificateDer, pem::PemObject};
use rustls::{ClientConfig, RootCertStore};

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[tokio::test(flavor = "multi_thread")]
async fn serves_return_handler_over_http3() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-return",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| http3_return_config(listen_addr, cert_path, key_path),
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));
    wait_for_admin_socket(
        &rginx_runtime::admin::admin_socket_path_for_config(server.config_path()),
        Duration::from_secs(5),
    );

    let response = http3_get(listen_addr, "localhost", "/v3", &cert.cert.pem())
        .await
        .expect("http3 request should succeed");

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(body_text(&response), "http3 return\n");

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn proxies_basic_http_requests_over_http3_to_http11_upstreams() {
    let upstream_listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("upstream listener should bind");
    upstream_listener
        .set_nonblocking(false)
        .expect("upstream listener should support blocking mode");
    let upstream_addr = upstream_listener.local_addr().expect("upstream addr should be available");
    let upstream_task = thread::spawn(move || {
        let (mut stream, _) =
            upstream_listener.accept().expect("upstream connection should arrive");
        let request = read_http_head_from_stream(&mut stream);
        assert!(
            request.starts_with("GET /demo HTTP/1.1\r\n"),
            "unexpected upstream request: {request}"
        );
        let body = "http3 proxy ok\n";
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .expect("upstream response should write");
        stream.flush().expect("upstream response should flush");
    });

    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-proxy",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| {
            http3_proxy_config(listen_addr, upstream_addr, cert_path, key_path)
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let response = http3_get(listen_addr, "localhost", "/api/demo", &cert.cert.pem())
        .await
        .expect("http3 proxy request should succeed");

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(body_text(&response), "http3 proxy ok\n");

    upstream_task.join().expect("upstream task should complete");
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn streams_http3_responses_without_buffering_entire_upstream_body() {
    let upstream_listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("upstream listener should bind");
    upstream_listener
        .set_nonblocking(false)
        .expect("upstream listener should support blocking mode");
    let upstream_addr = upstream_listener.local_addr().expect("upstream addr should be available");
    let upstream_task = thread::spawn(move || {
        let (mut stream, _) =
            upstream_listener.accept().expect("upstream connection should arrive");
        let request = read_http_head_from_stream(&mut stream);
        assert!(
            request.starts_with("GET /stream HTTP/1.1\r\n"),
            "unexpected upstream request: {request}"
        );
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
        )
        .expect("upstream response head should write");
        stream.flush().expect("upstream response head should flush");
        write_chunked_payload(&mut stream, b"first\n");
        thread::sleep(Duration::from_millis(900));
        write_chunked_payload(&mut stream, b"second\n");
        stream.write_all(b"0\r\n\r\n").expect("terminal chunk should write");
        stream.flush().expect("terminal chunk should flush");
    });

    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-streaming",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| {
            http3_streaming_proxy_config(listen_addr, upstream_addr, cert_path, key_path)
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let (status, first, second) =
        http3_streaming_get_two_chunks(listen_addr, "localhost", "/api/stream", &cert.cert.pem())
            .await
            .expect("http3 streaming request should succeed");

    assert_eq!(status, StatusCode::OK);
    assert_eq!(&first[..], b"first\n");
    assert_eq!(&second[..], b"second\n");

    upstream_task.join().expect("upstream task should complete");
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn enforces_access_control_and_rate_limits_over_http3() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-policy",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| http3_policy_config(listen_addr, cert_path, key_path),
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let allowed = http3_get(listen_addr, "localhost", "/allow", &cert.cert.pem())
        .await
        .expect("allowed request should succeed");
    assert_eq!(allowed.status, StatusCode::OK);
    assert_eq!(body_text(&allowed), "allowed\n");

    let denied = http3_get(listen_addr, "localhost", "/deny", &cert.cert.pem())
        .await
        .expect("denied request should receive a response");
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    assert_eq!(body_text(&denied), "forbidden\n");

    let first = http3_get(listen_addr, "localhost", "/limited", &cert.cert.pem())
        .await
        .expect("first limited request should succeed");
    assert_eq!(first.status, StatusCode::OK);
    let second = http3_get(listen_addr, "localhost", "/limited", &cert.cert.pem())
        .await
        .expect("second limited request should respond");
    assert_eq!(second.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(body_text(&second), "hold your horses! too many requests\n");

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn compresses_large_http3_responses_and_preserves_request_id_and_access_log() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-compression",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| http3_compression_config(listen_addr, cert_path, key_path),
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let response = http3_request(
        listen_addr,
        "localhost",
        "GET",
        "/gzip",
        &[("accept-encoding", "gzip"), ("x-request-id", "http3-log-42")],
        None,
        &cert.cert.pem(),
    )
    .await
    .expect("compressed request should succeed");

    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.header("content-encoding"), Some("gzip"));
    assert_eq!(response.header("vary"), Some("Accept-Encoding"));
    assert_eq!(response.header("x-request-id"), Some("http3-log-42"));
    assert_eq!(decode_gzip(&response.body), "http3 gzip body\n".repeat(32).into_bytes());

    server.shutdown_and_wait(Duration::from_secs(5));
    let logs = server.combined_output();
    assert!(
        logs.contains("H3 reqid=http3-log-42 version=HTTP/3.0 status=200"),
        "expected HTTP/3 access log entry, got {logs:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn traffic_command_counts_http3_requests() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-traffic",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| http3_return_config(listen_addr, cert_path, key_path),
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));
    wait_for_admin_socket(
        &rginx_runtime::admin::admin_socket_path_for_config(server.config_path()),
        Duration::from_secs(5),
    );

    let response = http3_get(listen_addr, "localhost", "/v3", &cert.cert.pem())
        .await
        .expect("http3 request should succeed");
    assert_eq!(response.status, StatusCode::OK);

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "traffic"]);
    assert!(output.status.success(), "traffic command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind=traffic_listener"));
    assert!(stdout.contains("kind=traffic_listener_http3"));
    assert!(stdout.contains("listener=default"));
    assert!(stdout.contains("downstream_requests_total=1"));
    assert!(stdout.contains("downstream_responses_total=1"));
    assert!(stdout.contains("retry_issued_total=0"));
    assert!(stdout.contains("request_body_stream_errors_total=0"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_responses_advertise_alt_svc_when_http3_is_enabled() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-alt-svc",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| http3_return_config(listen_addr, cert_path, key_path),
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let client = https_client(&cert.cert.pem());
    let request = Request::builder()
        .method("GET")
        .uri(format!("https://localhost:{}/v3", listen_addr.port()))
        .body(Empty::<Bytes>::new())
        .expect("https request should build");
    let response = client.request(request).await.expect("https request should succeed");
    let expected_alt_svc = format!("h3=\":{}\"; ma=7200", listen_addr.port());

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(hyper::http::header::ALT_SVC).and_then(|value| value.to_str().ok()),
        Some(expected_alt_svc.as_str())
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn required_client_auth_over_http3_accepts_authenticated_clients_and_rejects_anonymous_clients()
 {
    let fixture = Http3MtlsFixture::new("rginx-http3-required-mtls");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-required-mtls",
        &fixture.server_cert_pem,
        &fixture.server_key_pem,
        |temp_dir, cert_path, key_path| {
            let ca_path = temp_dir.join("client-ca.pem");
            fs::write(&ca_path, &fixture.ca_cert_pem).expect("CA cert should be written");
            http3_client_auth_config(listen_addr, cert_path, key_path, &ca_path, "Required")
        },
    );
    wait_for_admin_socket(
        &rginx_runtime::admin::admin_socket_path_for_config(server.config_path()),
        Duration::from_secs(5),
    );

    wait_for_http3_text_response(
        listen_addr,
        "localhost",
        "/v3",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
        200,
        "http3 mtls required\n",
        &fixture.ca_cert_pem,
        Duration::from_secs(5),
    )
    .await;

    let anonymous =
        http3_get_with_client_identity(listen_addr, "localhost", "/v3", None, &fixture.ca_cert_pem)
            .await;
    assert!(anonymous.is_err(), "anonymous HTTP/3 client should be rejected");

    let authenticated = http3_get_with_client_identity(
        listen_addr,
        "localhost",
        "/v3",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
        &fixture.ca_cert_pem,
    )
    .await
    .expect("authenticated HTTP/3 client should succeed");
    assert_eq!(authenticated.status, StatusCode::OK);
    assert_eq!(body_text(&authenticated), "http3 mtls required\n");

    let status_output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(
        status_output.status.success(),
        "status command should succeed: {}",
        render_output(&status_output)
    );
    let status_stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(status_stdout.contains("mtls_listeners=1"));
    assert!(status_stdout.contains("mtls_required_listeners=1"));
    assert!(parse_flat_u64(&status_stdout, "mtls_authenticated_connections") >= 1);
    assert!(parse_flat_u64(&status_stdout, "mtls_authenticated_requests") >= 1);
    assert!(parse_flat_u64(&status_stdout, "mtls_handshake_failures_missing_client_cert") >= 1);

    let counters_output =
        run_rginx(["--config", server.config_path().to_str().unwrap(), "counters"]);
    assert!(
        counters_output.status.success(),
        "counters command should succeed: {}",
        render_output(&counters_output)
    );
    let counters_stdout = String::from_utf8_lossy(&counters_output.stdout);
    assert!(
        parse_flat_u64(&counters_stdout, "downstream_mtls_authenticated_connections_total") >= 1
    );
    assert!(parse_flat_u64(&counters_stdout, "downstream_mtls_authenticated_requests_total") >= 1);
    assert!(
        parse_flat_u64(
            &counters_stdout,
            "downstream_tls_handshake_failures_missing_client_cert_total"
        ) >= 1
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn optional_client_auth_over_http3_allows_both_anonymous_and_authenticated_clients() {
    let fixture = Http3MtlsFixture::new("rginx-http3-optional-mtls");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-optional-mtls",
        &fixture.server_cert_pem,
        &fixture.server_key_pem,
        |temp_dir, cert_path, key_path| {
            let ca_path = temp_dir.join("client-ca.pem");
            fs::write(&ca_path, &fixture.ca_cert_pem).expect("CA cert should be written");
            http3_client_auth_config(listen_addr, cert_path, key_path, &ca_path, "Optional")
        },
    );

    wait_for_http3_text_response(
        listen_addr,
        "localhost",
        "/v3",
        None,
        200,
        "http3 mtls optional\n",
        &fixture.ca_cert_pem,
        Duration::from_secs(5),
    )
    .await;

    let anonymous =
        http3_get_with_client_identity(listen_addr, "localhost", "/v3", None, &fixture.ca_cert_pem)
            .await
            .expect("anonymous HTTP/3 client should succeed");
    assert_eq!(anonymous.status, StatusCode::OK);
    assert_eq!(body_text(&anonymous), "http3 mtls optional\n");

    let authenticated = http3_get_with_client_identity(
        listen_addr,
        "localhost",
        "/v3",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
        &fixture.ca_cert_pem,
    )
    .await
    .expect("authenticated HTTP/3 client should succeed");
    assert_eq!(authenticated.status, StatusCode::OK);
    assert_eq!(body_text(&authenticated), "http3 mtls optional\n");

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn validates_client_address_with_http3_retry_and_creates_host_key() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-retry",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |temp_dir, cert_path, key_path| {
            http3_retry_config(listen_addr, cert_path, key_path, &temp_dir.join("quic/host.key"))
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let response = http3_get(listen_addr, "localhost", "/v3", &cert.cert.pem())
        .await
        .expect("http3 request with retry should succeed");
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(body_text(&response), "http3 return\n");

    let status_output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(
        status_output.status.success(),
        "status command should succeed: {}",
        render_output(&status_output)
    );
    let status_stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(parse_flat_u64(&status_stdout, "http3_retry_issued_total") >= 1);
    assert!(status_stdout.contains("kind=status_listener_http3"));

    server.shutdown_and_wait(Duration::from_secs(5));

    let host_key_path = server.temp_dir().join("quic/host.key");
    let host_key = fs::read(&host_key_path).expect("http3 host key should be created");
    assert_eq!(host_key.len(), 64);

    let logs = server.combined_output();
    assert!(
        logs.contains("http3 issuing retry to validate client address"),
        "expected retry log entry, got {logs:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn preserves_http3_host_key_across_reload() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-retry-reload",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |temp_dir, cert_path, key_path| {
            http3_retry_config(listen_addr, cert_path, key_path, &temp_dir.join("quic/host.key"))
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let first = http3_get(listen_addr, "localhost", "/v3", &cert.cert.pem())
        .await
        .expect("initial http3 request should succeed");
    assert_eq!(first.status, StatusCode::OK);

    let host_key_path = server.temp_dir().join("quic/host.key");
    let before = fs::read(&host_key_path).expect("host key should exist before reload");
    assert_eq!(before.len(), 64);

    server.send_signal(libc::SIGHUP);
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let second = http3_get(listen_addr, "localhost", "/v3", &cert.cert.pem())
        .await
        .expect("http3 request after reload should succeed");
    assert_eq!(second.status, StatusCode::OK);

    let after = fs::read(&host_key_path).expect("host key should exist after reload");
    assert_eq!(before, after);

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread")]
async fn routes_http3_early_data_by_replay_safety() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-early-data",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| http3_early_data_config(listen_addr, cert_path, key_path),
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));
    wait_for_admin_socket(
        &rginx_runtime::admin::admin_socket_path_for_config(server.config_path()),
        Duration::from_secs(5),
    );

    let endpoint = http3_client_endpoint(None, &cert.cert.pem(), true)
        .expect("http3 early-data client should build");

    let (warmup, _) = http3_request_with_endpoint(
        &endpoint,
        listen_addr,
        "localhost",
        "GET",
        "/safe",
        &[],
        None,
        false,
        Duration::from_millis(150),
    )
    .await
    .expect("warmup request should succeed");
    assert_eq!(warmup.status, StatusCode::OK);
    assert_eq!(body_text(&warmup), "early data safe\n");

    let (safe, safe_accepted) = wait_for_http3_0rtt_request(
        &endpoint,
        listen_addr,
        "localhost",
        "/safe",
        Duration::from_secs(2),
    )
    .await
    .expect("0-RTT request to replay-safe route should succeed");
    assert!(safe_accepted, "server should accept 0-RTT data");
    assert_eq!(safe.status, StatusCode::OK);
    assert_eq!(body_text(&safe), "early data safe\n");

    let (unsafe_route, unsafe_accepted) = wait_for_http3_0rtt_request_status(
        &endpoint,
        listen_addr,
        "localhost",
        "/unsafe",
        StatusCode::TOO_EARLY,
        Duration::from_secs(2),
    )
    .await
    .expect("0-RTT request to non-replay-safe route should respond");
    assert!(unsafe_accepted, "server should keep 0-RTT enabled for the listener");
    assert_eq!(unsafe_route.status, StatusCode::TOO_EARLY);
    assert_eq!(body_text(&unsafe_route), "too early\n");

    endpoint.close(quinn::VarInt::from_u32(0), b"done");

    let status_output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(
        status_output.status.success(),
        "status command should succeed: {}",
        render_output(&status_output)
    );
    let status_stdout = String::from_utf8_lossy(&status_output.stdout);
    assert_eq!(parse_flat_u64(&status_stdout, "http3_early_data_enabled_listeners"), 1);
    assert!(parse_flat_u64(&status_stdout, "http3_early_data_rejected_requests") >= 1);

    let counters_output =
        run_rginx(["--config", server.config_path().to_str().unwrap(), "counters"]);
    assert!(
        counters_output.status.success(),
        "counters command should succeed: {}",
        render_output(&counters_output)
    );
    let counters_stdout = String::from_utf8_lossy(&counters_output.stdout);
    assert!(
        parse_flat_u64(&counters_stdout, "downstream_http3_early_data_rejected_requests_total")
            >= 1
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

struct Http3Response {
    status: StatusCode,
    headers: std::collections::HashMap<String, String>,
    body: Vec<u8>,
}

async fn http3_get(
    listen_addr: SocketAddr,
    server_name: &str,
    path: &str,
    cert_pem: &str,
) -> Result<Http3Response, String> {
    http3_request_inner(listen_addr, server_name, "GET", path, &[], None, None, cert_pem).await
}

async fn http3_get_with_client_identity(
    listen_addr: SocketAddr,
    server_name: &str,
    path: &str,
    client_identity: Option<(&Path, &Path)>,
    cert_pem: &str,
) -> Result<Http3Response, String> {
    http3_request_inner(listen_addr, server_name, "GET", path, &[], None, client_identity, cert_pem)
        .await
}

async fn http3_request(
    listen_addr: SocketAddr,
    server_name: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Bytes>,
    cert_pem: &str,
) -> Result<Http3Response, String> {
    http3_request_inner(listen_addr, server_name, method, path, headers, body, None, cert_pem).await
}

async fn http3_streaming_get_two_chunks(
    listen_addr: SocketAddr,
    server_name: &str,
    path: &str,
    cert_pem: &str,
) -> Result<(StatusCode, Bytes, Bytes), String> {
    let endpoint = http3_client_endpoint(None, cert_pem, false)?;
    let connection = endpoint
        .connect(listen_addr, server_name)
        .map_err(|error| format!("failed to start quic connect: {error}"))?
        .await
        .map_err(|error| format!("quic connect failed: {error}"))?;
    let connection_handle = connection.clone();

    let (mut driver, mut send_request) =
        client::new(h3_quinn::Connection::new(connection))
            .await
            .map_err(|error| format!("failed to initialize http3 client: {error}"))?;
    let mut driver_task = tokio::spawn(async move {
        let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });

    let request = Request::builder()
        .method("GET")
        .uri(format!("https://{server_name}:{}{path}", listen_addr.port()))
        .body(())
        .expect("http3 request should build");
    let mut request_stream = send_request
        .send_request(request)
        .await
        .map_err(|error| format!("failed to send http3 request: {error}"))?;
    request_stream
        .finish()
        .await
        .map_err(|error| format!("failed to finish http3 request: {error}"))?;

    let response = request_stream
        .recv_response()
        .await
        .map_err(|error| format!("failed to receive http3 response headers: {error}"))?;
    let status = response.status();
    let mut first = tokio::time::timeout(Duration::from_millis(500), request_stream.recv_data())
        .await
        .map_err(|_| "timed out waiting for first http3 response chunk".to_string())?
        .map_err(|error| format!("failed to receive first http3 response chunk: {error}"))?
        .ok_or_else(|| "http3 response body ended before the first chunk arrived".to_string())?;
    let mut second = tokio::time::timeout(Duration::from_secs(2), request_stream.recv_data())
        .await
        .map_err(|_| "timed out waiting for second http3 response chunk".to_string())?
        .map_err(|error| format!("failed to receive second http3 response chunk: {error}"))?
        .ok_or_else(|| "http3 response body ended before the second chunk arrived".to_string())?;

    while request_stream
        .recv_data()
        .await
        .map_err(|error| format!("failed to drain remaining http3 response body: {error}"))?
        .is_some()
    {}
    let _ = request_stream
        .recv_trailers()
        .await
        .map_err(|error| format!("failed to receive http3 response trailers: {error}"))?;

    connection_handle.close(quinn::VarInt::from_u32(0), b"done");
    endpoint.close(quinn::VarInt::from_u32(0), b"done");
    if tokio::time::timeout(Duration::from_millis(50), &mut driver_task).await.is_err() {
        driver_task.abort();
        let _ = driver_task.await;
    }

    Ok((status, first.copy_to_bytes(first.remaining()), second.copy_to_bytes(second.remaining())))
}

async fn http3_request_inner(
    listen_addr: SocketAddr,
    server_name: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Bytes>,
    client_identity: Option<(&Path, &Path)>,
    cert_pem: &str,
) -> Result<Http3Response, String> {
    let endpoint = http3_client_endpoint(client_identity, cert_pem, false)?;
    let (response, _) = http3_request_with_endpoint(
        &endpoint,
        listen_addr,
        server_name,
        method,
        path,
        headers,
        body,
        false,
        Duration::ZERO,
    )
    .await?;
    endpoint.close(quinn::VarInt::from_u32(0), b"done");

    Ok(response)
}

fn http3_client_endpoint(
    client_identity: Option<(&Path, &Path)>,
    cert_pem: &str,
    enable_early_data: bool,
) -> Result<quinn::Endpoint, String> {
    let roots = root_store_from_pem(cert_pem)?;
    let client_crypto = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|error| format!("failed to constrain TLS versions for http3 client: {error}"))?
    .with_root_certificates(roots);
    let mut client_crypto = match client_identity {
        Some((cert_path, key_path)) => {
            let certs = load_certs_from_path(cert_path)?;
            let key = load_private_key_from_path(key_path)?;
            client_crypto
                .with_client_auth_cert(certs, key)
                .map_err(|error| format!("failed to configure HTTP/3 client cert: {error}"))?
        }
        None => client_crypto.with_no_client_auth(),
    };
    client_crypto.alpn_protocols = vec![b"h3".to_vec()];
    client_crypto.enable_early_data = enable_early_data;

    let client_config = quinn::ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(client_crypto)
            .map_err(|error| format!("failed to build quic client config: {error}"))?,
    ));
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap())
        .map_err(|error| error.to_string())?;
    endpoint.set_default_client_config(client_config);

    Ok(endpoint)
}

#[allow(clippy::too_many_arguments)]
async fn http3_request_with_endpoint(
    endpoint: &quinn::Endpoint,
    listen_addr: SocketAddr,
    server_name: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Bytes>,
    use_0rtt: bool,
    linger_after_response: Duration,
) -> Result<(Http3Response, Option<bool>), String> {
    let connecting = endpoint
        .connect(listen_addr, server_name)
        .map_err(|error| format!("failed to start quic connect: {error}"))?;
    let (connection, zero_rtt_accepted) = if use_0rtt {
        match connecting.into_0rtt() {
            Ok((connection, accepted)) => (connection, Some(accepted)),
            Err(_) => return Err("0-RTT resumption was not available".to_string()),
        }
    } else {
        let connection =
            connecting.await.map_err(|error| format!("quic connect failed: {error}"))?;
        (connection, None)
    };
    let connection_handle = connection.clone();

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
    let mut body = BytesMut::new();
    while let Some(chunk) = request_stream
        .recv_data()
        .await
        .map_err(|error| format!("failed to receive http3 response body: {error}"))?
    {
        body.extend_from_slice(chunk.chunk());
    }
    let _ = request_stream
        .recv_trailers()
        .await
        .map_err(|error| format!("failed to receive http3 response trailers: {error}"))?;

    let early_data_accepted = match zero_rtt_accepted {
        Some(accepted) => Some(accepted.await),
        None => None,
    };

    if linger_after_response > Duration::ZERO {
        tokio::time::sleep(linger_after_response).await;
    }

    connection_handle.close(quinn::VarInt::from_u32(0), b"done");

    if tokio::time::timeout(Duration::from_millis(50), &mut driver_task).await.is_err() {
        driver_task.abort();
        let _ = driver_task.await;
    }

    Ok((Http3Response { status, headers, body: body.to_vec() }, early_data_accepted))
}

async fn wait_for_http3_0rtt_request(
    endpoint: &quinn::Endpoint,
    listen_addr: SocketAddr,
    server_name: &str,
    path: &str,
    timeout: Duration,
) -> Result<(Http3Response, bool), String> {
    let deadline = std::time::Instant::now() + timeout;
    let mut last_error = String::new();

    while std::time::Instant::now() < deadline {
        match http3_request_with_endpoint(
            endpoint,
            listen_addr,
            server_name,
            "GET",
            path,
            &[],
            None,
            true,
            Duration::ZERO,
        )
        .await
        {
            Ok((response, Some(accepted))) => return Ok((response, accepted)),
            Ok((_response, None)) => {
                last_error =
                    "0-RTT request unexpectedly completed without an acceptance signal".to_string();
            }
            Err(error) if error.contains("0-RTT resumption was not available") => {
                last_error = error;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error),
        }
    }

    Err(format!(
        "timed out waiting for reusable 0-RTT state for https://{server_name}:{}{path}; last error: {last_error}",
        listen_addr.port()
    ))
}

async fn wait_for_http3_0rtt_request_status(
    endpoint: &quinn::Endpoint,
    listen_addr: SocketAddr,
    server_name: &str,
    path: &str,
    expected_status: StatusCode,
    timeout: Duration,
) -> Result<(Http3Response, bool), String> {
    let deadline = std::time::Instant::now() + timeout;
    let mut last_error = String::new();

    while std::time::Instant::now() < deadline {
        match wait_for_http3_0rtt_request(
            endpoint,
            listen_addr,
            server_name,
            path,
            Duration::from_millis(250),
        )
        .await
        {
            Ok((response, accepted)) if response.status == expected_status => {
                return Ok((response, accepted));
            }
            Ok((response, accepted)) => {
                last_error = format!(
                    "0-RTT request completed with status={} accepted={} instead of {}",
                    response.status, accepted, expected_status
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(error) => {
                last_error = error;
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
    }

    Err(format!(
        "timed out waiting for 0-RTT request with status {} for https://{server_name}:{}{path}; last error: {last_error}",
        expected_status,
        listen_addr.port()
    ))
}

fn https_client(
    cert_pem: &str,
) -> Client<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
    Empty<Bytes>,
> {
    let roots = root_store_from_pem(cert_pem).expect("root store should build");
    let client_config = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("https client should support default protocol versions")
    .with_root_certificates(roots)
    .with_no_client_auth();
    let connector = HttpsConnectorBuilder::new()
        .with_tls_config(client_config)
        .https_only()
        .enable_all_versions()
        .build();
    Client::builder(TokioExecutor::new()).build(connector)
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

fn load_certs_from_path(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    CertificateDer::pem_file_iter(path)
        .map_err(|error| format!("failed to open cert `{}`: {error}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to parse cert `{}`: {error}", path.display()))
}

fn load_private_key_from_path(
    path: &Path,
) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    rustls::pki_types::PrivateKeyDer::from_pem_file(path)
        .map_err(|error| format!("failed to parse key `{}`: {error}", path.display()))
}

fn http3_return_config(
    listen_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"fallback\\n\"),\n            ),\n        ),\n    ],\n    servers: [\n        VirtualHostConfig(\n            server_names: [\"localhost\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/v3\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"http3 return\\n\"),\n                    ),\n                ),\n            ],\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn http3_policy_config(
    listen_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/allow\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"allowed\\n\"),\n            ),\n            allow_cidrs: [\"127.0.0.1/32\", \"::1/128\"],\n        ),\n        LocationConfig(\n            matcher: Exact(\"/deny\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"denied\\n\"),\n            ),\n            deny_cidrs: [\"127.0.0.1/32\", \"::1/128\"],\n        ),\n        LocationConfig(\n            matcher: Exact(\"/limited\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"limited\\n\"),\n            ),\n            requests_per_sec: Some(1),\n            burst: Some(0),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn http3_compression_config(
    listen_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        access_log_format: Some(\"H3 reqid=$request_id version=$http_version status=$status\"),\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/gzip\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({:?}),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        "http3 gzip body\n".repeat(32),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn http3_proxy_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [UpstreamPeerConfig(url: {:?})],\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n                preserve_host: Some(false),\n                strip_prefix: Some(\"/api\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn http3_streaming_proxy_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [UpstreamPeerConfig(url: {:?})],\n            request_timeout_secs: Some(5),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n                preserve_host: Some(false),\n                strip_prefix: Some(\"/api\"),\n            ),\n            response_buffering: Some(Off),\n            compression: Some(Off),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn http3_retry_config(
    listen_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
    host_key_path: &std::path::Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n            active_connection_id_limit: Some(5),\n            retry: Some(true),\n            host_key_path: Some({:?}),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"fallback\\n\"),\n            ),\n        ),\n    ],\n    servers: [\n        VirtualHostConfig(\n            server_names: [\"localhost\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/v3\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"http3 return\\n\"),\n                    ),\n                ),\n            ],\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        host_key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn http3_early_data_config(
    listen_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n            early_data: Some(true),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/safe\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"early data safe\\n\"),\n            ),\n            allow_early_data: Some(true),\n        ),\n        LocationConfig(\n            matcher: Exact(\"/unsafe\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"early data unsafe\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn http3_client_auth_config(
    listen_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
    ca_path: &std::path::Path,
    mode: &str,
) -> String {
    let body = if mode == "Required" { "http3 mtls required\n" } else { "http3 mtls optional\n" };
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n            client_auth: Some(ServerClientAuthConfig(\n                mode: {},\n                ca_cert_path: {:?},\n            )),\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/v3\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({:?}),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        mode,
        ca_path.display().to_string(),
        body,
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn generate_cert(hostname: &str) -> rcgen::CertifiedKey<rcgen::KeyPair> {
    rcgen::generate_simple_self_signed(vec![hostname.to_string()])
        .expect("self-signed certificate should generate")
}

struct Http3MtlsFixture {
    _dir: PathBuf,
    ca_cert_pem: String,
    server_cert_pem: String,
    server_key_pem: String,
    client_cert_path: PathBuf,
    client_key_path: PathBuf,
}

impl Http3MtlsFixture {
    fn new(prefix: &str) -> Self {
        let dir = temp_dir(prefix);
        fs::create_dir_all(&dir).expect("fixture temp dir should be created");

        let ca = generate_ca_cert("rginx h3 mtls ca");
        let server =
            generate_cert_signed_by_ca("localhost", &ca, ExtendedKeyUsagePurpose::ServerAuth);
        let client = generate_cert_signed_by_ca(
            "client.example.com",
            &ca,
            ExtendedKeyUsagePurpose::ClientAuth,
        );

        let client_cert_path = dir.join("client.crt");
        let client_key_path = dir.join("client.key");
        fs::write(&client_cert_path, client.cert.pem()).expect("client cert should be written");
        fs::write(&client_key_path, client.signing_key.serialize_pem())
            .expect("client key should be written");

        Self {
            _dir: dir,
            ca_cert_pem: ca.cert.pem(),
            server_cert_pem: server.cert.pem(),
            server_key_pem: server.signing_key.serialize_pem(),
            client_cert_path,
            client_key_path,
        }
    }
}

fn temp_dir(prefix: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!("{}/{}-{}-{}", std::env::temp_dir().display(), prefix, unique, id))
}

struct TestCertifiedKey {
    cert: rcgen::Certificate,
    signing_key: KeyPair,
    params: CertificateParams,
}

impl TestCertifiedKey {
    fn issuer(&self) -> Issuer<'_, &KeyPair> {
        Issuer::from_params(&self.params, &self.signing_key)
    }
}

fn generate_ca_cert(common_name: &str) -> TestCertifiedKey {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name.push(DnType::CommonName, common_name);
    let signing_key = KeyPair::generate().expect("CA keypair should generate");
    let cert = params.self_signed(&signing_key).expect("CA cert should generate");
    TestCertifiedKey { cert, signing_key, params }
}

fn generate_cert_signed_by_ca(
    dns_name: &str,
    issuer: &TestCertifiedKey,
    usage: ExtendedKeyUsagePurpose,
) -> TestCertifiedKey {
    let mut params =
        CertificateParams::new(vec![dns_name.to_string()]).expect("leaf params should build");
    params.distinguished_name.push(DnType::CommonName, dns_name);
    params.extended_key_usages = vec![usage];
    let signing_key = KeyPair::generate().expect("leaf keypair should generate");
    let cert =
        params.signed_by(&signing_key, &issuer.issuer()).expect("leaf cert should be signed");
    TestCertifiedKey { cert, signing_key, params }
}

fn decode_gzip(bytes: &[u8]) -> Vec<u8> {
    let mut decoder = GzDecoder::new(bytes);
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).expect("gzip body should decode");
    decoded
}

fn body_text(response: &Http3Response) -> String {
    String::from_utf8(response.body.clone()).expect("response body should be valid UTF-8")
}

impl Http3Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }
}

fn wait_for_admin_socket(path: &Path, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if path.exists() && UnixStream::connect(path).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }

    panic!("timed out waiting for admin socket {}", path.display());
}

async fn wait_for_http3_text_response(
    listen_addr: SocketAddr,
    server_name: &str,
    path: &str,
    client_identity: Option<(&Path, &Path)>,
    expected_status: u16,
    expected_body: &str,
    cert_pem: &str,
    timeout: Duration,
) {
    let deadline = std::time::Instant::now() + timeout;
    let mut last_error = String::new();

    while std::time::Instant::now() < deadline {
        match http3_get_with_client_identity(
            listen_addr,
            server_name,
            path,
            client_identity,
            cert_pem,
        )
        .await
        {
            Ok(response)
                if response.status.as_u16() == expected_status
                    && body_text(&response) == expected_body =>
            {
                return;
            }
            Ok(response) => {
                last_error = format!(
                    "unexpected response: status={} body={:?}",
                    response.status.as_u16(),
                    body_text(&response)
                );
            }
            Err(error) => last_error = error,
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!(
        "timed out waiting for expected HTTP/3 response on {listen_addr}{path}; last error: {last_error}"
    );
}

fn parse_flat_u64(output: &str, key: &str) -> u64 {
    output
        .split_whitespace()
        .find_map(|field| field.strip_prefix(&format!("{key}=")))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

fn run_rginx(args: impl IntoIterator<Item = impl AsRef<str>>) -> std::process::Output {
    let mut command = std::process::Command::new(binary_path());
    for arg in args {
        command.arg(arg.as_ref());
    }
    command.output().expect("rginx command should run")
}

fn binary_path() -> std::path::PathBuf {
    std::env::var_os("CARGO_BIN_EXE_rginx")
        .map(std::path::PathBuf::from)
        .expect("cargo should expose the rginx test binary path")
}

fn render_output(output: &std::process::Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn read_http_head_from_stream(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 256];

    loop {
        let read = stream.read(&mut chunk).expect("HTTP head should be readable");
        assert!(read > 0, "stream closed before HTTP head was complete");
        buffer.extend_from_slice(&chunk[..read]);

        if let Some(head_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            return String::from_utf8(buffer[..head_end + 4].to_vec())
                .expect("HTTP head should be valid UTF-8");
        }
    }
}

fn write_chunked_payload(stream: &mut std::net::TcpStream, chunk: &[u8]) {
    write!(stream, "{:x}\r\n", chunk.len()).expect("chunk header should write");
    stream.write_all(chunk).expect("chunk payload should write");
    stream.write_all(b"\r\n").expect("chunk terminator should write");
    stream.flush().expect("chunk should flush");
}
