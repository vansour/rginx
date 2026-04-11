#![cfg(unix)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, read_http_head, reserve_loopback_addr};

const TEST_SERVER_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIDCTCCAfGgAwIBAgIUE+LKmhgfKie/YU/anMKv+Xgr5dYwDQYJKoZIhvcNAQEL\nBQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDMyMDE1MzIzMloXDTI2MDMy\nMTE1MzIzMlowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF\nAAOCAQ8AMIIBCgKCAQEAvxn1IYqOORs2Ys/6Ou54G3alu+wZOeGkPy/ZLYUuO0pK\nh1WgvPvwGF3w3XZdEPhB0JXhqwqoz60SwGQJtEM9GGRHVnBV+BeE/4L1XO4H6Gz5\npMKFaCcJPwO4IrspjffpKQ217K9l9vbjK31tJKwOGaQ//icyzF13xuUvZms67PNc\nBqhZQchld9s90InnL3fCS+J58s9pjE0qlTr7bodvOXaYBxboDlBh4YV7PW/wjwBo\ngUwcbiJvtrRnY7ZlRi/C/bZUTGJ5kO7vSlAgMh2KL1DyY2Ws06n5KUNgpAuIjmew\nMtuYJ9H2xgRMrMjgWSD8N/RRFut4xnpm7jlRepzvwwIDAQABo1MwUTAdBgNVHQ4E\nFgQUIezWZPz8VZj6n2znyGWv76RsGMswHwYDVR0jBBgwFoAUIezWZPz8VZj6n2zn\nyGWv76RsGMswDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAbngq\np7KT2JaXL8BYQGThBZwRODtqv/jXwc34zE3DPPRb1F3i8/odH7+9ZLse35Hj0/gp\nqFQ0DNdOuNlrbrvny208P1OcBe2hYWOSsRGyhZpM5Ai+DkuHheZfhNKvWKdbFn8+\nyfeyN3orSsin9QG0Yx3eqtO/1/6D5TtLsnY2/yPV/j0pv2GCCuB0kcKfygOQTYW6\nJrmYzeFeR/bnQM/lOM49leURdgC/x7tveNG7KRvD0X85M9iuT9/0+VSu6yAkcEi5\nx23C/Chzu7FFVxwZRHD+RshbV4QTPewhi17EJwroMYFpjGUHJVUfzo6W6bsWqA59\nCiiHI87NdBZv4JUCOQ==\n-----END CERTIFICATE-----\n";
const TEST_SERVER_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC/GfUhio45GzZi\nz/o67ngbdqW77Bk54aQ/L9kthS47SkqHVaC8+/AYXfDddl0Q+EHQleGrCqjPrRLA\nZAm0Qz0YZEdWcFX4F4T/gvVc7gfobPmkwoVoJwk/A7giuymN9+kpDbXsr2X29uMr\nfW0krA4ZpD/+JzLMXXfG5S9mazrs81wGqFlByGV32z3Qiecvd8JL4nnyz2mMTSqV\nOvtuh285dpgHFugOUGHhhXs9b/CPAGiBTBxuIm+2tGdjtmVGL8L9tlRMYnmQ7u9K\nUCAyHYovUPJjZazTqfkpQ2CkC4iOZ7Ay25gn0fbGBEysyOBZIPw39FEW63jGembu\nOVF6nO/DAgMBAAECggEAKLC7v80TVHiFX4veQZ8WRu7AAmAWzPrNMMEc8rLZcblz\nXhau956DdITILTevQFZEGUhYuUU3RaUaCYojgNUSVLfBctfPjlhfstItMYDjgSt3\nCox6wH8TWm4NzqNgiUCgzmODeaatROUz4MY/r5/NDsuo7pJlIBvEzb5uFdY+QUZ/\nR5gHRiD2Q3wCODe8zQRfTZGo7jCimAuWTLurWZl6ax/4TjWbXCD6DTuUo81cW3vy\nne6tEetHcABRO7uDoBYXk12pCgqFZzjLMnKJjQM+OYnSj6DoWjOu1drT5YyRLGDj\nfzN8V0aKRkOYoZ5QZOua8pByOyQElJnM16vkPtHgPQKBgQD6SOUNWEghvYIGM/lx\nc22/zjvDjeaGC3qSmlpQYN5MGuDoszeDBZ+rMTmHqJ9FcHYkLQnUI7ZkHhRGt/wQ\n/w3CroJjPBgKk+ipy2cBHSI+z+U20xjYzE8hxArWbXG1G4rDt5AIz68IQPsfkVND\nktkDABDaU+KwBPx8fjeeqtRQxQKBgQDDdxdLB1XcfZMX0KEP5RfA8ar1nW41TUAl\nTCOLaXIQbHZ0BeW7USE9mK8OKnVALZGJ+rpxvYFPZ5MWxchpb/cuIwXjLoN6uZVb\nfx4Hho+2iCfhcEKzs8XZW48duKIfhx13BiILLf/YaHAWFs9UfVcQog4Qx03guyMr\n7k9bFuy25wKBgQDpE48zAT6TJS775dTrAQp4b28aan/93pyz/8gRSFRb3UALlDIi\n8s7BluKzYaWI/fUXNVYM14EX9Sb+wIGdtlezL94+2Yyt9RXbYY8361Cj2+jiSG3A\nH2ulzzIkg+E7Pj3Yi443lmiysAjsWeKHcC5l697F4w6cytfye3wCZ6W23QKBgQC0\n9tX+5aytdSkwnDvxXlVOka+ItBcri/i+Ty59TMOIxxInuqoFcUhIIcq4X8CsCUQ8\nLYBd+2fznt3D8JrqWvnKoiw6N38MqTLJQfgIWaFGCep6QhfPDbo30RfAGYcnj01N\nO8Va+lxq+84B9V5AR8bKpG5HRG4qiLc4XerkV2YSswKBgDt9eerSBZyLVwfku25Y\nfrh+nEjUZy81LdlpJmu/bfa2FfItzBqDZPskkJJW9ON82z/ejGFbsU48RF7PJUMr\nGimE33QeTDToGozHCq0QOd0SMfsVkOQR+EROdmY52UIYAYgQUfI1FQ9lLsw10wlQ\nD11SHTL7b9pefBWfW73I7ttV\n-----END PRIVATE KEY-----\n";

#[test]
fn serves_the_same_routes_on_explicit_http_and_https_listeners() {
    let http_addr = reserve_loopback_addr();
    let https_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-multi-listener",
        TEST_SERVER_CERT_PEM,
        TEST_SERVER_KEY_PEM,
        |_, cert_path, key_path| {
            multi_listener_config(http_addr, https_addr, cert_path, key_path, None)
        },
    );

    server.wait_for_http_ready(http_addr, Duration::from_secs(5));
    server.wait_for_https_ready(https_addr, Duration::from_secs(5));
    server.wait_for_http_text_response(
        http_addr,
        "example.com",
        "/",
        200,
        "multi listener\n",
        Duration::from_secs(5),
    );
    server.wait_for_https_text_response(
        https_addr,
        "example.com",
        "/",
        "localhost",
        200,
        "multi listener\n",
        Duration::from_secs(5),
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn max_connections_are_scoped_per_listener() {
    let http_addr = reserve_loopback_addr();
    let https_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-multi-listener-max-connections",
        TEST_SERVER_CERT_PEM,
        TEST_SERVER_KEY_PEM,
        |_, cert_path, key_path| {
            multi_listener_config(http_addr, https_addr, cert_path, key_path, Some(1))
        },
    );

    server.wait_for_http_ready(http_addr, Duration::from_secs(5));
    server.wait_for_https_ready(https_addr, Duration::from_secs(5));

    let mut held = TcpStream::connect_timeout(&http_addr, Duration::from_millis(200))
        .expect("http client should connect");
    held.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    held.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
    write!(held, "GET / HTTP/1.1\r\nHost: example.com\r\nConnection: keep-alive\r\n\r\n").unwrap();
    held.flush().unwrap();
    let head = read_http_head(&mut held);
    assert!(head.starts_with("HTTP/1.1 200"), "unexpected held response: {head:?}");

    server.wait_for_https_text_response(
        https_addr,
        "example.com",
        "/",
        "localhost",
        200,
        "multi listener\n",
        Duration::from_secs(5),
    );

    let mut extra = TcpStream::connect_timeout(&http_addr, Duration::from_millis(200))
        .expect("second http client should connect before being rejected");
    extra.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    extra.set_write_timeout(Some(Duration::from_millis(500))).unwrap();
    let write_rejected =
        match write!(extra, "GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n") {
            Ok(()) => match extra.flush() {
                Ok(()) => false,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::BrokenPipe
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::UnexpectedEof
                    ) =>
                {
                    true
                }
                Err(error) => {
                    panic!("expected second http connection to close cleanly, got {error}")
                }
            },
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::UnexpectedEof
                ) =>
            {
                true
            }
            Err(error) => panic!("expected second http connection to close cleanly, got {error}"),
        };

    if !write_rejected {
        let mut buffer = [0u8; 64];
        match extra.read(&mut buffer) {
            Ok(0) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::BrokenPipe
                        | std::io::ErrorKind::ConnectionReset
                        | std::io::ErrorKind::UnexpectedEof
                ) => {}
            Ok(read) => panic!(
                "expected second http connection to be closed, received {:?}",
                String::from_utf8_lossy(&buffer[..read])
            ),
            Err(error) => panic!("expected second http connection to close cleanly, got {error}"),
        }
    }

    drop(extra);
    drop(held);
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn listener_specific_access_log_formats_are_honored() {
    let http_addr = reserve_loopback_addr();
    let https_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-multi-listener-access-log",
        TEST_SERVER_CERT_PEM,
        TEST_SERVER_KEY_PEM,
        |_, cert_path, key_path| {
            multi_listener_access_log_config(http_addr, https_addr, cert_path, key_path)
        },
    );

    server.wait_for_http_ready(http_addr, Duration::from_secs(5));
    server.wait_for_https_ready(https_addr, Duration::from_secs(5));

    let http_response =
        send_http_request(http_addr, "example.com", "/", "http-log-42").expect("http request");
    assert!(
        http_response.starts_with("HTTP/1.1 200"),
        "unexpected http response: {http_response:?}"
    );

    let https_response =
        send_https_request(https_addr, "example.com", "/", "https-log-42").expect("https request");
    assert!(
        https_response.starts_with("HTTP/1.1 200"),
        "unexpected https response: {https_response:?}"
    );

    server.terminate_and_wait(Duration::from_secs(5));

    let output = server.combined_output();
    assert!(
        output.contains("ACCESS listener=http rid=http-log-42"),
        "expected http listener access log line\n{output}"
    );
    assert!(
        output.contains("ACCESS listener=https rid=https-log-42"),
        "expected https listener access log line\n{output}"
    );
}

fn multi_listener_config(
    http_addr: SocketAddr,
    https_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
    max_connections: Option<u64>,
) -> String {
    let max_connections = max_connections
        .map(|limit| format!("            max_connections: Some({limit}),\n"))
        .unwrap_or_default();
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    listeners: [\n        ListenerConfig(\n            name: \"http\",\n            listen: {:?},\n{max_connections}        ),\n        ListenerConfig(\n            name: \"https\",\n            listen: {:?},\n{max_connections}            tls: Some(ServerTlsConfig(\n                cert_path: {:?},\n                key_path: {:?},\n            )),\n        ),\n    ],\n    server: ServerConfig(\n        server_names: [\"example.com\"],\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"multi listener\\n\"),\n            ),\n        ),\n    ],\n)\n",
        http_addr.to_string(),
        https_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
        max_connections = max_connections,
    )
}

fn multi_listener_access_log_config(
    http_addr: SocketAddr,
    https_addr: SocketAddr,
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    listeners: [\n        ListenerConfig(\n            name: \"http\",\n            listen: {:?},\n            access_log_format: Some(\"ACCESS listener=http rid=$request_id\"),\n        ),\n        ListenerConfig(\n            name: \"https\",\n            listen: {:?},\n            access_log_format: Some(\"ACCESS listener=https rid=$request_id\"),\n            tls: Some(ServerTlsConfig(\n                cert_path: {:?},\n                key_path: {:?},\n            )),\n        ),\n    ],\n    server: ServerConfig(\n        server_names: [\"example.com\"],\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"multi listener\\n\"),\n            ),\n        ),\n    ],\n)\n",
        http_addr.to_string(),
        https_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn send_http_request(
    listen_addr: SocketAddr,
    host: &str,
    path: &str,
    request_id: &str,
) -> Result<String, String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nX-Request-ID: {request_id}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|error| format!("failed to write request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush request: {error}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("failed to read response: {error}"))?;
    Ok(response)
}

fn send_https_request(
    listen_addr: SocketAddr,
    host: &str,
    path: &str,
    request_id: &str,
) -> Result<String, String> {
    let tcp = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    tcp.set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;

    let mut config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(InsecureServerCertVerifier::new()))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let server_name = ServerName::try_from("localhost".to_string())
        .map_err(|error| format!("invalid TLS server name: {error}"))?;
    let connection = ClientConnection::new(Arc::new(config), server_name)
        .map_err(|error| format!("failed to build TLS client: {error}"))?;
    let mut stream = StreamOwned::new(connection, tcp);

    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nX-Request-ID: {request_id}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|error| format!("failed to write HTTPS request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush HTTPS request: {error}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("failed to read HTTPS response: {error}"))?;
    Ok(response)
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
