use std::convert::Infallible;
use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::header::HOST;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{DigitallySignedStruct, DistinguishedName, ProtocolVersion, SignatureScheme};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

const TEST_SERVER_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIDCTCCAfGgAwIBAgIUE+LKmhgfKie/YU/anMKv+Xgr5dYwDQYJKoZIhvcNAQEL\nBQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDMyMDE1MzIzMloXDTI2MDMy\nMTE1MzIzMlowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF\nAAOCAQ8AMIIBCgKCAQEAvxn1IYqOORs2Ys/6Ou54G3alu+wZOeGkPy/ZLYUuO0pK\nh1WgvPvwGF3w3XZdEPhB0JXhqwqoz60SwGQJtEM9GGRHVnBV+BeE/4L1XO4H6Gz5\npMKFaCcJPwO4IrspjffpKQ217K9l9vbjK31tJKwOGaQ//icyzF13xuUvZms67PNc\nBqhZQchld9s90InnL3fCS+J58s9pjE0qlTr7bodvOXaYBxboDlBh4YV7PW/wjwBo\ngUwcbiJvtrRnY7ZlRi/C/bZUTGJ5kO7vSlAgMh2KL1DyY2Ws06n5KUNgpAuIjmew\nMtuYJ9H2xgRMrMjgWSD8N/RRFut4xnpm7jlRepzvwwIDAQABo1MwUTAdBgNVHQ4EFgQUIezWZPz8VZj6n2znyGWv76RsGMswHwYDVR0jBBgwFoAUIezWZPz8VZj6n2znyGWv76RsGMswDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAbngqp7KT2JaXL8BYQGThBZwRODtqv/jXwc34zE3DPPRb1F3i8/odH7+9ZLse35Hj0/gpqFQ0DNdOuNlrbrvny208P1OcBe2hYWOSsRGyhZpM5Ai+DkuHheZfhNKvWKdbFn8+yfeyN3orSsin9QG0Yx3eqtO/1/6D5TtLsnY2/yPV/j0pv2GCCuB0kcKfygOQTYW6JrmYzeFeR/bnQM/lOM49leURdgC/x7tveNG7KRvD0X85M9iuT9/0+VSu6yAkcEi5x23C/Chzu7FFVxwZRHD+RshbV4QTPewhi17EJwroMYFpjGUHJVUfzo6W6bsWqA59CiiHI87NdBZv4JUCOQ==\n-----END CERTIFICATE-----\n";
const TEST_SERVER_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC/GfUhio45GzZi\nz/o67ngbdqW77Bk54aQ/L9kthS47SkqHVaC8+/AYXfDddl0Q+EHQleGrCqjPrRLA\nZAm0Qz0YZEdWcFX4F4T/gvVc7gfobPmkwoVoJwk/A7giuymN9+kpDbXsr2X29uMr\nfW0krA4ZpD/+JzLMXXfG5S9mazrs81wGqFlByGV32z3Qiecvd8JL4nnyz2mMTSqV\nOvtuh285dpgHFugOUGHhhXs9b/CPAGiBTBxuIm+2tGdjtmVGL8L9tlRMYnmQ7u9K\nUCAyHYovUPJjZazTqfkpQ2CkC4iOZ7Ay25gn0fbGBEysyOBZIPw39FEW63jGembu\nOVF6nO/DAgMBAAECggEAKLC7v80TVHiFX4veQZ8WRu7AAmAWzPrNMMEc8rLZcblz\nXhau956DdITILTevQFZEGUhYuUU3RaUaCYojgNUSVLfBctfPjlhfstItMYDjgSt3\nCox6wH8TWm4NzqNgiUCgzmODeaatROUz4MY/r5/NDsuo7pJlIBvEzb5uFdY+QUZ/\nR5gHRiD2Q3wCODe8zQRfTZGo7jCimAuWTLurWZl6ax/4TjWbXCD6DTuUo81cW3vy\nne6tEetHcABRO7uDoBYXk12pCgqFZzjLMnKJjQM+OYnSj6DoWjOu1drT5YyRLGDj\nfzN8V0aKRkOYoZ5QZOua8pByOyQElJnM16vkPtHgPQKBgQD6SOUNWEghvYIGM/lx\nc22/zjvDjeaGC3qSmlpQYN5MGuDoszeDBZ+rMTmHqJ9FcHYkLQnUI7ZkHhRGt/wQ\n/w3CroJjPBgKk+ipy2cBHSI+z+U20xjYzE8hxArWbXG1G4rDt5AIz68IQPsfkVND\nktkDABDaU+KwBPx8fjeeqtRQxQKBgQDDdxdLB1XcfZMX0KEP5RfA8ar1nW41TUAl\nTCOLaXIQbHZ0BeW7USE9mK8OKnVALZGJ+rpxvYFPZ5MWxchpb/cuIwXjLoN6uZVb\nfx4Hho+2iCfhcEKzs8XZW48duKIfhx13BiILLf/YaHAWFs9UfVcQog4Qx03guyMr\n7k9bFuy25wKBgQDpE48zAT6TJS775dTrAQp4b28aan/93pyz/8gRSFRb3UALlDIi\n8s7BluKzYaWI/fUXNVYM14EX9Sb+wIGdtlezL94+2Yyt9RXbYY8361Cj2+jiSG3A\nH2ulzzIkg+E7Pj3Yi443lmiysAjsWeKHcC5l697F4w6cytfye3wCZ6W23QKBgQC0\n9tX+5aytdSkwnDvxXlVOka+ItBcri/i+Ty59TMOIxxInuqoFcUhIIcq4X8CsCUQ8\nLYBd+2fznt3D8JrqWvnKoiw6N38MqTLJQfgIWaFGCep6QhfPDbo30RfAGYcnj01N\nO8Va+lxq+84B9V5AR8bKpG5HRG4qiLc4XerkV2YSswKBgDt9eerSBZyLVwfku25Y\nfrh+nEjUZy81LdlpJmu/bfa2FfItzBqDZPskkJJW9ON82z/ejGFbsU48RF7PJUMr\nGimE33QeTDToGozHCq0QOd0SMfsVkOQR+EROdmY52UIYAYgQUfI1FQ9lLsw10wlQD11SHTL7b9pefBWfW73I7ttV\n-----END PRIVATE KEY-----\n";

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedMtlsRequest {
    peer_certificates_present: bool,
    protocol_version: Option<ProtocolVersion>,
    host: Option<String>,
}

#[tokio::test(flavor = "multi_thread")]
async fn proxies_to_https_upstreams_with_client_certificate_and_tls13() {
    let shared_dir = temp_dir("rginx-upstream-mtls-shared");
    fs::create_dir_all(&shared_dir).expect("shared temp dir should be created");
    let client_cert_path = shared_dir.join("client.crt");
    let client_key_path = shared_dir.join("client.key");
    fs::write(&client_cert_path, TEST_SERVER_CERT_PEM).expect("client cert should be written");
    fs::write(&client_key_path, TEST_SERVER_KEY_PEM).expect("client key should be written");

    let (upstream_addr, observed_rx, upstream_task, upstream_temp_dir) =
        spawn_mtls_upstream(client_cert_path.clone(), client_key_path.clone()).await;

    let listen_addr = reserve_loopback_addr();
    let client_cert = client_cert_path.clone();
    let client_key = client_key_path.clone();
    let mut server = ServerHarness::spawn("rginx-upstream-mtls", |_| {
        proxy_config(listen_addr, upstream_addr, &client_cert, &client_key)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/",
        200,
        "upstream mtls ok\n",
        Duration::from_secs(5),
    );

    let observed = tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .expect("upstream request should be observed before timeout")
        .expect("upstream observation channel should complete");
    assert!(observed.peer_certificates_present);
    assert_eq!(observed.protocol_version, Some(ProtocolVersion::TLSv1_3));
    // The upstream is h1, so Hyper must rebuild Host from URI authority if Auto+HTTPS stripped it.
    let expected_host = format!("127.0.0.1:{}", upstream_addr.port());
    assert_eq!(observed.host.as_deref(), Some(expected_host.as_str()));

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream mTLS server task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
    fs::remove_dir_all(shared_dir).expect("shared temp dir should be removed");
}

async fn spawn_mtls_upstream(
    _client_ca_path: PathBuf,
    _client_key_path: PathBuf,
) -> (SocketAddr, oneshot::Receiver<ObservedMtlsRequest>, JoinHandle<()>, PathBuf) {
    let temp_dir = temp_dir("rginx-upstream-mtls-server");
    fs::create_dir_all(&temp_dir).expect("upstream temp dir should be created");
    let cert_path = temp_dir.join("upstream.crt");
    let key_path = temp_dir.join("upstream.key");
    fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("upstream cert should be written");
    fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("upstream key should be written");

    let certs = load_certs(&cert_path);
    let key = load_private_key(&key_path);
    let verifier = Arc::new(RequireAnyClientCertVerifier::new());
    let tls_config =
        rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .with_client_cert_verifier(verifier)
            .with_single_cert(certs, key)
            .expect("test upstream TLS config should build");
    let tls_acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("upstream mTLS listener should bind");
    let listen_addr = listener.local_addr().expect("upstream mTLS addr should be available");
    let (observed_tx, observed_rx) = oneshot::channel();
    let observed_tx = Arc::new(Mutex::new(Some(observed_tx)));

    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("upstream mTLS listener should accept");
        let tls_stream =
            tls_acceptor.accept(stream).await.expect("upstream TLS handshake should work");
        let peer_certificates_present =
            tls_stream.get_ref().1.peer_certificates().is_some_and(|certs| !certs.is_empty());
        let protocol_version = tls_stream.get_ref().1.protocol_version();

        let service = service_fn(move |request: Request<Incoming>| {
            let observed_tx = observed_tx.clone();

            async move {
                let host = request
                    .headers()
                    .get(HOST)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                if let Some(sender) =
                    observed_tx.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()
                {
                    let _ = sender.send(ObservedMtlsRequest {
                        peer_certificates_present,
                        protocol_version,
                        host,
                    });
                }

                Ok::<_, Infallible>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .header("content-type", "text/plain; charset=utf-8")
                        .body(Full::new(Bytes::from_static(b"upstream mtls ok\n")))
                        .expect("upstream response should build"),
                )
            }
        });

        http1::Builder::new()
            .serve_connection(TokioIo::new(tls_stream), service)
            .await
            .expect("upstream mTLS connection should complete");
    });

    (listen_addr, observed_rx, task, temp_dir)
}

fn proxy_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    client_cert_path: &Path,
    client_key_path: &Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(UpstreamTlsConfig(\n                verify: Insecure,\n                versions: Some([Tls13]),\n                client_cert_path: Some({:?}),\n                client_key_path: Some({:?}),\n            )),\n            protocol: Auto,\n            server_name_override: Some(\"localhost\"),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        client_cert_path.display().to_string(),
        client_key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
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

#[derive(Debug)]
struct RequireAnyClientCertVerifier {
    root_hints: Vec<DistinguishedName>,
    supported_schemes: Vec<SignatureScheme>,
}

impl RequireAnyClientCertVerifier {
    fn new() -> Self {
        Self {
            root_hints: Vec::new(),
            supported_schemes: rustls::crypto::aws_lc_rs::default_provider()
                .signature_verification_algorithms
                .supported_schemes(),
        }
    }
}

impl ClientCertVerifier for RequireAnyClientCertVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &self.root_hints
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.supported_schemes.clone()
    }
}
