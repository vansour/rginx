use std::io::{ErrorKind, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::{BodyExt, Empty};
use hyper::{Request, StatusCode, Version};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};

mod support;

use support::{
    READY_ROUTE_CONFIG, ServerHarness, apply_tls_placeholders, read_http_head,
    reserve_loopback_addr,
};

const STREAMING_CHUNK_DEADLINE: Duration = Duration::from_millis(1000);

const TEST_SERVER_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIDCTCCAfGgAwIBAgIUE+LKmhgfKie/YU/anMKv+Xgr5dYwDQYJKoZIhvcNAQEL\nBQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDMyMDE1MzIzMloXDTI2MDMy\nMTE1MzIzMlowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF\nAAOCAQ8AMIIBCgKCAQEAvxn1IYqOORs2Ys/6Ou54G3alu+wZOeGkPy/ZLYUuO0pK\nh1WgvPvwGF3w3XZdEPhB0JXhqwqoz60SwGQJtEM9GGRHVnBV+BeE/4L1XO4H6Gz5\npMKFaCcJPwO4IrspjffpKQ217K9l9vbjK31tJKwOGaQ//icyzF13xuUvZms67PNc\nBqhZQchld9s90InnL3fCS+J58s9pjE0qlTr7bodvOXaYBxboDlBh4YV7PW/wjwBo\ngUwcbiJvtrRnY7ZlRi/C/bZUTGJ5kO7vSlAgMh2KL1DyY2Ws06n5KUNgpAuIjmew\nMtuYJ9H2xgRMrMjgWSD8N/RRFut4xnpm7jlRepzvwwIDAQABo1MwUTAdBgNVHQ4E\nFgQUIezWZPz8VZj6n2znyGWv76RsGMswHwYDVR0jBBgwFoAUIezWZPz8VZj6n2zn\nyGWv76RsGMswDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAbngq\np7KT2JaXL8BYQGThBZwRODtqv/jXwc34zE3DPPRb1F3i8/odH7+9ZLse35Hj0/gp\nqFQ0DNdOuNlrbrvny208P1OcBe2hYWOSsRGyhZpM5Ai+DkuHheZfhNKvWKdbFn8+\nyfeyN3orSsin9QG0Yx3eqtO/1/6D5TtLsnY2/yPV/j0pv2GCCuB0kcKfygOQTYW6\nJrmYzeFeR/bnQM/lOM49leURdgC/x7tveNG7KRvD0X85M9iuT9/0+VSu6yAkcEi5\nx23C/Chzu7FFVxwZRHD+RshbV4QTPewhi17EJwroMYFpjGUHJVUfzo6W6bsWqA59\nCiiHI87NdBZv4JUCOQ==\n-----END CERTIFICATE-----\n";
const TEST_SERVER_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC/GfUhio45GzZi\nz/o67ngbdqW77Bk54aQ/L9kthS47SkqHVaC8+/AYXfDddl0Q+EHQleGrCqjPrRLA\nZAm0Qz0YZEdWcFX4F4T/gvVc7gfobPmkwoVoJwk/A7giuymN9+kpDbXsr2X29uMr\nfW0krA4ZpD/+JzLMXXfG5S9mazrs81wGqFlByGV32z3Qiecvd8JL4nnyz2mMTSqV\nOvtuh285dpgHFugOUGHhhXs9b/CPAGiBTBxuIm+2tGdjtmVGL8L9tlRMYnmQ7u9K\nUCAyHYovUPJjZazTqfkpQ2CkC4iOZ7Ay25gn0fbGBEysyOBZIPw39FEW63jGembu\nOVF6nO/DAgMBAAECggEAKLC7v80TVHiFX4veQZ8WRu7AAmAWzPrNMMEc8rLZcblz\nXhau956DdITILTevQFZEGUhYuUU3RaUaCYojgNUSVLfBctfPjlhfstItMYDjgSt3\nCox6wH8TWm4NzqNgiUCgzmODeaatROUz4MY/r5/NDsuo7pJlIBvEzb5uFdY+QUZ/\nR5gHRiD2Q3wCODe8zQRfTZGo7jCimAuWTLurWZl6ax/4TjWbXCD6DTuUo81cW3vy\nne6tEetHcABRO7uDoBYXk12pCgqFZzjLMnKJjQM+OYnSj6DoWjOu1drT5YyRLGDj\nfzN8V0aKRkOYoZ5QZOua8pByOyQElJnM16vkPtHgPQKBgQD6SOUNWEghvYIGM/lx\nc22/zjvDjeaGC3qSmlpQYN5MGuDoszeDBZ+rMTmHqJ9FcHYkLQnUI7ZkHhRGt/wQ\n/w3CroJjPBgKk+ipy2cBHSI+z+U20xjYzE8hxArWbXG1G4rDt5AIz68IQPsfkVND\nktkDABDaU+KwBPx8fjeeqtRQxQKBgQDDdxdLB1XcfZMX0KEP5RfA8ar1nW41TUAl\nTCOLaXIQbHZ0BeW7USE9mK8OKnVALZGJ+rpxvYFPZ5MWxchpb/cuIwXjLoN6uZVb\nfx4Hho+2iCfhcEKzs8XZW48duKIfhx13BiILLf/YaHAWFs9UfVcQog4Qx03guyMr\n7k9bFuy25wKBgQDpE48zAT6TJS775dTrAQp4b28aan/93pyz/8gRSFRb3UALlDIi\n8s7BluKzYaWI/fUXNVYM14EX9Sb+wIGdtlezL94+2Yyt9RXbYY8361Cj2+jiSG3A\nH2ulzzIkg+E7Pj3Yi443lmiysAjsWeKHcC5l697F4w6cytfye3wCZ6W23QKBgQC0\n9tX+5aytdSkwnDvxXlVOka+ItBcri/i+Ty59TMOIxxInuqoFcUhIIcq4X8CsCUQ8\nLYBd+2fznt3D8JrqWvnKoiw6N38MqTLJQfgIWaFGCep6QhfPDbo30RfAGYcnj01N\nO8Va+lxq+84B9V5AR8bKpG5HRG4qiLc4XerkV2YSswKBgDt9eerSBZyLVwfku25Y\nfrh+nEjUZy81LdlpJmu/bfa2FfItzBqDZPskkJJW9ON82z/ejGFbsU48RF7PJUMr\nGimE33QeTDToGozHCq0QOd0SMfsVkOQR+EROdmY52UIYAYgQUfI1FQ9lLsw10wlQ\nD11SHTL7b9pefBWfW73I7ttV\n-----END PRIVATE KEY-----\n";

#[tokio::test(flavor = "multi_thread")]
async fn serves_http2_over_tls_and_proxies_to_http11_upstreams() {
    let upstream_listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("upstream listener should bind");
    upstream_listener
        .set_nonblocking(true)
        .expect("upstream listener should support nonblocking mode");
    let upstream_addr =
        upstream_listener.local_addr().expect("upstream listener addr should be available");
    let upstream_task = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match upstream_listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    panic!("timed out waiting for upstream connection: {error}");
                }
                Err(error) => panic!("failed to accept upstream connection: {error}"),
            }
        };

        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("upstream read timeout should be configurable");
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("upstream write timeout should be configurable");

        let request = read_http_head(&mut stream);
        assert!(
            request.starts_with("GET / HTTP/1.1\r\n"),
            "expected proxied HTTP/2 request to be normalized to HTTP/1.1 upstream, got {request:?}"
        );

        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 11\r\nConnection: close\r\n\r\nbackend ok\n",
            )
            .expect("upstream should write response");
        stream.flush().expect("upstream response should flush");
    });

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http2-test",
        TEST_SERVER_CERT_PEM,
        TEST_SERVER_KEY_PEM,
        |_, cert_path, key_path| {
            apply_tls_placeholders(
                tls_proxy_config(listen_addr, upstream_addr),
                cert_path,
                key_path,
            )
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let connector = HttpsConnectorBuilder::new()
        .with_tls_config(
            ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(InsecureServerCertVerifier::new()))
                .with_no_client_auth(),
        )
        .https_only()
        .enable_http2()
        .build();
    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

    let request = Request::builder()
        .uri(format!("https://127.0.0.1:{}/", listen_addr.port()))
        .version(Version::HTTP_2)
        .body(Empty::<Bytes>::new())
        .expect("HTTP/2 request should build");
    let response = tokio::time::timeout(Duration::from_secs(5), client.request(request))
        .await
        .expect("HTTP/2 request should not time out")
        .expect("HTTP/2 request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.version(), Version::HTTP_2);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("HTTP/2 response body should collect")
        .to_bytes();
    assert_eq!(&body[..], b"backend ok\n");

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.join().expect("upstream thread should complete");
}

#[tokio::test(flavor = "multi_thread")]
async fn streams_http2_responses_without_buffering_entire_upstream_body() {
    let upstream_listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("upstream listener should bind");
    upstream_listener
        .set_nonblocking(true)
        .expect("upstream listener should support nonblocking mode");
    let upstream_addr =
        upstream_listener.local_addr().expect("upstream listener addr should be available");
    let upstream_task = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match upstream_listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    panic!("timed out waiting for upstream connection: {error}");
                }
                Err(error) => panic!("failed to accept upstream connection: {error}"),
            }
        };

        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("upstream read timeout should be configurable");
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .expect("upstream write timeout should be configurable");

        let request = read_http_head(&mut stream);
        assert!(
            request.starts_with("GET /stream HTTP/1.1\r\n"),
            "expected proxied HTTP/2 request to preserve the streaming path, got {request:?}"
        );

        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .expect("upstream response head should write");
        write_chunk(&mut stream, b"first\n");
        thread::sleep(Duration::from_millis(1500));
        write_chunk(&mut stream, b"second\n");
        stream.write_all(b"0\r\n\r\n").expect("terminal chunk should write");
        stream.flush().expect("terminal chunk should flush");
    });

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http2-streaming-test",
        TEST_SERVER_CERT_PEM,
        TEST_SERVER_KEY_PEM,
        |_, cert_path, key_path| {
            apply_tls_placeholders(
                tls_streaming_proxy_config(listen_addr, upstream_addr),
                cert_path,
                key_path,
            )
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let connector = HttpsConnectorBuilder::new()
        .with_tls_config(
            ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(InsecureServerCertVerifier::new()))
                .with_no_client_auth(),
        )
        .https_only()
        .enable_http2()
        .build();
    let client: Client<_, Empty<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

    let request = Request::builder()
        .uri(format!("https://127.0.0.1:{}/api/stream", listen_addr.port()))
        .version(Version::HTTP_2)
        .body(Empty::<Bytes>::new())
        .expect("HTTP/2 request should build");
    let response = tokio::time::timeout(Duration::from_secs(5), client.request(request))
        .await
        .expect("HTTP/2 request should not time out")
        .expect("HTTP/2 request should succeed");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.version(), Version::HTTP_2);

    let headers_received_at = Instant::now();
    let mut body = response.into_body();
    let first = tokio::time::timeout(STREAMING_CHUNK_DEADLINE, next_data_chunk(&mut body))
        .await
        .expect("first HTTP/2 chunk should arrive before upstream completes")
        .expect("first HTTP/2 chunk should exist");
    assert_eq!(&first[..], b"first\n");
    assert!(
        headers_received_at.elapsed() < STREAMING_CHUNK_DEADLINE,
        "first HTTP/2 chunk should arrive soon after response headers, elapsed={:?}",
        headers_received_at.elapsed()
    );

    let second = tokio::time::timeout(Duration::from_secs(2), next_data_chunk(&mut body))
        .await
        .expect("second HTTP/2 chunk should arrive")
        .expect("second HTTP/2 chunk should exist");
    assert_eq!(&second[..], b"second\n");
    assert!(next_data_chunk(&mut body).await.is_none(), "stream should end after two chunks");

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.join().expect("upstream thread should complete");
}

fn tls_proxy_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        tls: Some(ServerTlsConfig(\n            cert_path: \"__CERT_PATH__\",\n            key_path: \"__KEY_PATH__\",\n        )),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn tls_streaming_proxy_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        tls: Some(ServerTlsConfig(\n            cert_path: \"__CERT_PATH__\",\n            key_path: \"__KEY_PATH__\",\n        )),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            request_timeout_secs: Some(5),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n                preserve_host: Some(false),\n                strip_prefix: Some(\"/api\"),\n            ),\n            response_buffering: Some(Off),\n            compression: Some(Off),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}

async fn next_data_chunk<B>(body: &mut B) -> Option<Bytes>
where
    B: hyper::body::Body<Data = Bytes> + Unpin,
    B::Error: std::fmt::Display,
{
    while let Some(frame) = body.frame().await {
        let frame =
            frame.unwrap_or_else(|error| panic!("streaming body frame should arrive: {error}"));
        if let Ok(data) = frame.into_data() {
            if data.is_empty() {
                continue;
            }
            return Some(data);
        }
    }
    None
}

fn write_chunk(stream: &mut std::net::TcpStream, chunk: &[u8]) {
    write!(stream, "{:x}\r\n", chunk.len()).expect("chunk header should write");
    stream.write_all(chunk).expect("chunk payload should write");
    stream.write_all(b"\r\n").expect("chunk terminator should write");
    stream.flush().expect("chunk should flush");
}

#[derive(Debug)]
struct InsecureServerCertVerifier {
    supported_schemes: Vec<SignatureScheme>,
}

impl InsecureServerCertVerifier {
    fn new() -> Self {
        Self {
            supported_schemes: rustls::crypto::aws_lc_rs::default_provider()
                .signature_verification_algorithms
                .supported_schemes(),
        }
    }
}

impl ServerCertVerifier for InsecureServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.supported_schemes.clone()
    }
}
