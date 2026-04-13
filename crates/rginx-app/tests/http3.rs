use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
use flate2::read::GzDecoder;
use h3::client;
use http_body_util::Empty;
use hyper::http::{Request, StatusCode};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use quinn::crypto::rustls::QuicClientConfig;
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
    assert!(stdout.contains("listener=default"));
    assert!(stdout.contains("downstream_requests_total=1"));
    assert!(stdout.contains("downstream_responses_total=1"));

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
    http3_request(listen_addr, server_name, "GET", path, &[], None, cert_pem).await
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

    if tokio::time::timeout(Duration::from_millis(50), &mut driver_task).await.is_err() {
        driver_task.abort();
        let _ = driver_task.await;
    }
    endpoint.close(quinn::VarInt::from_u32(0), b"done");

    Ok(Http3Response { status, headers, body: body.to_vec() })
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

fn generate_cert(hostname: &str) -> rcgen::CertifiedKey<rcgen::KeyPair> {
    rcgen::generate_simple_self_signed(vec![hostname.to_string()])
        .expect("self-signed certificate should generate")
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
