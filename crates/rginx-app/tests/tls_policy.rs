use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{
    ClientConfig, ClientConnection, DigitallySignedStruct, ProtocolVersion, SignatureScheme,
    StreamOwned,
};

mod support;

use support::{ServerHarness, apply_tls_placeholders, reserve_loopback_addr};

const TEST_SERVER_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIDCTCCAfGgAwIBAgIUE+LKmhgfKie/YU/anMKv+Xgr5dYwDQYJKoZIhvcNAQEL\nBQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDMyMDE1MzIzMloXDTI2MDMy\nMTE1MzIzMlowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF\nAAOCAQ8AMIIBCgKCAQEAvxn1IYqOORs2Ys/6Ou54G3alu+wZOeGkPy/ZLYUuO0pK\nh1WgvPvwGF3w3XZdEPhB0JXhqwqoz60SwGQJtEM9GGRHVnBV+BeE/4L1XO4H6Gz5\npMKFaCcJPwO4IrspjffpKQ217K9l9vbjK31tJKwOGaQ//icyzF13xuUvZms67PNc\nBqhZQchld9s90InnL3fCS+J58s9pjE0qlTr7bodvOXaYBxboDlBh4YV7PW/wjwBo\ngUwcbiJvtrRnY7ZlRi/C/bZUTGJ5kO7vSlAgMh2KL1DyY2Ws06n5KUNgpAuIjmew\nMtuYJ9H2xgRMrMjgWSD8N/RRFut4xnpm7jlRepzvwwIDAQABo1MwUTAdBgNVHQ4E\nFgQUIezWZPz8VZj6n2znyGWv76RsGMswHwYDVR0jBBgwFoAUIezWZPz8VZj6n2zn\nyGWv76RsGMswDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAbngq\np7KT2JaXL8BYQGThBZwRODtqv/jXwc34zE3DPPRb1F3i8/odH7+9ZLse35Hj0/gp\nqFQ0DNdOuNlrbrvny208P1OcBe2hYWOSsRGyhZpM5Ai+DkuHheZfhNKvWKdbFn8+\nyfeyN3orSsin9QG0Yx3eqtO/1/6D5TtLsnY2/yPV/j0pv2GCCuB0kcKfygOQTYW6\nJrmYzeFeR/bnQM/lOM49leURdgC/x7tveNG7KRvD0X85M9iuT9/0+VSu6yAkcEi5\nx23C/Chzu7FFVxwZRHD+RshbV4QTPewhi17EJwroMYFpjGUHJVUfzo6W6bsWqA59\nCiiHI87NdBZv4JUCOQ==\n-----END CERTIFICATE-----\n";
const TEST_SERVER_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC/GfUhio45GzZi\nz/o67ngbdqW77Bk54aQ/L9kthS47SkqHVaC8+/AYXfDddl0Q+EHQleGrCqjPrRLA\nZAm0Qz0YZEdWcFX4F4T/gvVc7gfobPmkwoVoJwk/A7giuymN9+kpDbXsr2X29uMr\nfW0krA4ZpD/+JzLMXXfG5S9mazrs81wGqFlByGV32z3Qiecvd8JL4nnyz2mMTSqV\nOvtuh285dpgHFugOUGHhhXs9b/CPAGiBTBxuIm+2tGdjtmVGL8L9tlRMYnmQ7u9K\nUCAyHYovUPJjZazTqfkpQ2CkC4iOZ7Ay25gn0fbGBEysyOBZIPw39FEW63jGembu\nOVF6nO/DAgMBAAECggEAKLC7v80TVHiFX4veQZ8WRu7AAmAWzPrNMMEc8rLZcblz\nXhau956DdITILTevQFZEGUhYuUU3RaUaCYojgNUSVLfBctfPjlhfstItMYDjgSt3\nCox6wH8TWm4NzqNgiUCgzmODeaatROUz4MY/r5/NDsuo7pJlIBvEzb5uFdY+QUZ/\nR5gHRiD2Q3wCODe8zQRfTZGo7jCimAuWTLurWZl6ax/4TjWbXCD6DTuUo81cW3vy\nne6tEetHcABRO7uDoBYXk12pCgqFZzjLMnKJjQM+OYnSj6DoWjOu1drT5YyRLGDj\nfzN8V0aKRkOYoZ5QZOua8pByOyQElJnM16vkPtHgPQKBgQD6SOUNWEghvYIGM/lx\nc22/zjvDjeaGC3qSmlpQYN5MGuDoszeDBZ+rMTmHqJ9FcHYkLQnUI7ZkHhRGt/wQ\n/w3CroJjPBgKk+ipy2cBHSI+z+U20xjYzE8hxArWbXG1G4rDt5AIz68IQPsfkVND\nktkDABDaU+KwBPx8fjeeqtRQxQKBgQDDdxdLB1XcfZMX0KEP5RfA8ar1nW41TUAl\nTCOLaXIQbHZ0BeW7USE9mK8OKnVALZGJ+rpxvYFPZ5MWxchpb/cuIwXjLoN6uZVb\nfx4Hho+2iCfhcEKzs8XZW48duKIfhx13BiILLf/YaHAWFs9UfVcQog4Qx03guyMr\n7k9bFuy25wKBgQDpE48zAT6TJS775dTrAQp4b28aan/93pyz/8gRSFRb3UALlDIi\n8s7BluKzYaWI/fUXNVYM14EX9Sb+wIGdtlezL94+2Yyt9RXbYY8361Cj2+jiSG3A\nH2ulzzIkg+E7Pj3Yi443lmiysAjsWeKHcC5l697F4w6cytfye3wCZ6W23QKBgQC0\n9tX+5aytdSkwnDvxXlVOka+ItBcri/i+Ty59TMOIxxInuqoFcUhIIcq4X8CsCUQ8\nLYBd+2fznt3D8JrqWvnKoiw6N38MqTLJQfgIWaFGCep6QhfPDbo30RfAGYcnj01N\nO8Va+lxq+84B9V5AR8bKpG5HRG4qiLc4XerkV2YSswKBgDt9eerSBZyLVwfku25Y\nfrh+nEjUZy81LdlpJmu/bfa2FfItzBqDZPskkJJW9ON82z/ejGFbsU48RF7PJUMr\nGimE33QeTDToGozHCq0QOd0SMfsVkOQR+EROdmY52UIYAYgQUfI1FQ9lLsw10wlQ\nD11SHTL7b9pefBWfW73I7ttV\n-----END PRIVATE KEY-----\n";

#[test]
fn tls12_only_listener_negotiates_tls12() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-tls12-only",
        TEST_SERVER_CERT_PEM,
        TEST_SERVER_KEY_PEM,
        |_, cert_path, key_path| {
            apply_tls_placeholders(tls12_only_config(listen_addr), cert_path, key_path)
        },
    );

    wait_for_https_policy_response(
        &mut server,
        listen_addr,
        TlsPolicyCase {
            host: "localhost",
            path: "/",
            server_name: "localhost",
            enable_sni: true,
            alpn_protocols: &[b"http/1.1".as_slice()],
            expected_status: 200,
            expected_body: "tls12 only\n",
            expected_protocol_version: Some(ProtocolVersion::TLSv1_2),
            expected_alpn: Some("http/1.1"),
        },
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn custom_alpn_protocols_disable_h2_negotiation() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-alpn-http11",
        TEST_SERVER_CERT_PEM,
        TEST_SERVER_KEY_PEM,
        |_, cert_path, key_path| {
            apply_tls_placeholders(http11_only_alpn_config(listen_addr), cert_path, key_path)
        },
    );

    wait_for_https_policy_response(
        &mut server,
        listen_addr,
        TlsPolicyCase {
            host: "localhost",
            path: "/",
            server_name: "localhost",
            enable_sni: true,
            alpn_protocols: &[b"h2".as_slice(), b"http/1.1".as_slice()],
            expected_status: 200,
            expected_body: "http11 alpn\n",
            expected_protocol_version: None,
            expected_alpn: Some("http/1.1"),
        },
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn default_certificate_supports_sniless_clients_with_multiple_vhost_certs() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-default-cert-fallback",
        TEST_SERVER_CERT_PEM,
        TEST_SERVER_KEY_PEM,
        |_, cert_path, key_path| {
            apply_tls_placeholders(
                sniless_default_certificate_config(listen_addr),
                cert_path,
                key_path,
            )
        },
    );

    wait_for_https_policy_response(
        &mut server,
        listen_addr,
        TlsPolicyCase {
            host: "api.example.com",
            path: "/",
            server_name: "localhost",
            enable_sni: false,
            alpn_protocols: &[b"http/1.1".as_slice()],
            expected_status: 200,
            expected_body: "api root\n",
            expected_protocol_version: None,
            expected_alpn: Some("http/1.1"),
        },
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

struct TlsPolicyResponse {
    status: u16,
    body: String,
    protocol_version: Option<ProtocolVersion>,
    alpn_protocol: Option<String>,
}

struct TlsPolicyCase<'a> {
    host: &'a str,
    path: &'a str,
    server_name: &'a str,
    enable_sni: bool,
    alpn_protocols: &'a [&'a [u8]],
    expected_status: u16,
    expected_body: &'a str,
    expected_protocol_version: Option<ProtocolVersion>,
    expected_alpn: Option<&'a str>,
}

fn wait_for_https_policy_response(
    server: &mut ServerHarness,
    listen_addr: SocketAddr,
    case: TlsPolicyCase<'_>,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last_error = String::new();

    while Instant::now() < deadline {
        server.assert_running();

        match fetch_https_policy_response(
            listen_addr,
            case.host,
            case.path,
            case.server_name,
            case.enable_sni,
            case.alpn_protocols,
        ) {
            Ok(response)
                if response.status == case.expected_status
                    && response.body == case.expected_body
                    && case
                        .expected_protocol_version
                        .is_none_or(|version| response.protocol_version == Some(version))
                    && case.expected_alpn.is_none_or(|protocol| {
                        response.alpn_protocol.as_deref() == Some(protocol)
                    }) =>
            {
                return;
            }
            Ok(response) => {
                last_error = format!(
                    "unexpected response: status={} body={:?} tls={:?} alpn={:?}",
                    response.status,
                    response.body,
                    response.protocol_version,
                    response.alpn_protocol
                );
            }
            Err(error) => last_error = error,
        }

        thread::sleep(Duration::from_millis(50));
    }

    panic!(
        "timed out waiting for TLS policy response on https://{listen_addr}{}; last error: {}\n{}",
        case.path,
        last_error,
        server.combined_output()
    );
}

fn fetch_https_policy_response(
    listen_addr: SocketAddr,
    host: &str,
    path: &str,
    server_name: &str,
    enable_sni: bool,
    alpn_protocols: &[&[u8]],
) -> Result<TlsPolicyResponse, String> {
    let tcp = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    tcp.set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    tcp.set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;

    let mut config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(InsecureServerCertVerifier::new()))
        .with_no_client_auth();
    config.enable_sni = enable_sni;
    config.alpn_protocols = alpn_protocols.iter().map(|protocol| protocol.to_vec()).collect();

    let server_name = ServerName::try_from(server_name.to_string())
        .map_err(|error| format!("invalid TLS server name `{server_name}`: {error}"))?;
    let connection = ClientConnection::new(Arc::new(config), server_name)
        .map_err(|error| format!("failed to build TLS client: {error}"))?;
    let mut stream = StreamOwned::new(connection, tcp);

    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
        .map_err(|error| format!("failed to write HTTPS request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush HTTPS request: {error}"))?;

    let protocol_version = stream.conn.protocol_version();
    let alpn_protocol =
        stream.conn.alpn_protocol().map(|protocol| String::from_utf8_lossy(protocol).into_owned());

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("failed to read HTTPS response: {error}"))?;

    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| format!("malformed response: {response:?}"))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| format!("missing status line: {head:?}"))?
        .parse::<u16>()
        .map_err(|error| format!("invalid status code: {error}"))?;

    Ok(TlsPolicyResponse { status, body: body.to_string(), protocol_version, alpn_protocol })
}

fn tls12_only_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        tls: Some(ServerTlsConfig(\n            cert_path: \"__CERT_PATH__\",\n            key_path: \"__KEY_PATH__\",\n            versions: Some([Tls12]),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"tls12 only\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
    )
}

fn http11_only_alpn_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        tls: Some(ServerTlsConfig(\n            cert_path: \"__CERT_PATH__\",\n            key_path: \"__KEY_PATH__\",\n            alpn_protocols: Some([\"http/1.1\"]),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"http11 alpn\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
    )
}

fn sniless_default_certificate_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        default_certificate: Some(\"api.example.com\"),\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"default root\\n\"),\n            ),\n        ),\n    ],\n    servers: [\n        VirtualHostConfig(\n            server_names: [\"api.example.com\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"api root\\n\"),\n                    ),\n                ),\n            ],\n            tls: Some(VirtualHostTlsConfig(\n                cert_path: \"__CERT_PATH__\",\n                key_path: \"__KEY_PATH__\",\n            )),\n        ),\n        VirtualHostConfig(\n            server_names: [\"other.example.com\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"other root\\n\"),\n                    ),\n                ),\n            ],\n            tls: Some(VirtualHostTlsConfig(\n                cert_path: \"__CERT_PATH__\",\n                key_path: \"__KEY_PATH__\",\n            )),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
    )
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
