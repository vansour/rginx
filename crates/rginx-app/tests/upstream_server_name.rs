use std::convert::Infallible;
use std::env;
use std::fmt;
use std::fs::{self, File};
use std::io::BufReader;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use rustls::crypto::aws_lc_rs;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

const TEST_SERVER_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIDCTCCAfGgAwIBAgIUE+LKmhgfKie/YU/anMKv+Xgr5dYwDQYJKoZIhvcNAQEL\nBQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDMyMDE1MzIzMloXDTI2MDMy\nMTE1MzIzMlowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF\nAAOCAQ8AMIIBCgKCAQEAvxn1IYqOORs2Ys/6Ou54G3alu+wZOeGkPy/ZLYUuO0pK\nh1WgvPvwGF3w3XZdEPhB0JXhqwqoz60SwGQJtEM9GGRHVnBV+BeE/4L1XO4H6Gz5\npMKFaCcJPwO4IrspjffpKQ217K9l9vbjK31tJKwOGaQ//icyzF13xuUvZms67PNc\nBqhZQchld9s90InnL3fCS+J58s9pjE0qlTr7bodvOXaYBxboDlBh4YV7PW/wjwBo\ngUwcbiJvtrRnY7ZlRi/C/bZUTGJ5kO7vSlAgMh2KL1DyY2Ws06n5KUNgpAuIjmew\nMtuYJ9H2xgRMrMjgWSD8N/RRFut4xnpm7jlRepzvwwIDAQABo1MwUTAdBgNVHQ4E\nFgQUIezWZPz8VZj6n2znyGWv76RsGMswHwYDVR0jBBgwFoAUIezWZPz8VZj6n2zn\nyGWv76RsGMswDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAbngq\np7KT2JaXL8BYQGThBZwRODtqv/jXwc34zE3DPPRb1F3i8/odH7+9ZLse35Hj0/gp\nqFQ0DNdOuNlrbrvny208P1OcBe2hYWOSsRGyhZpM5Ai+DkuHheZfhNKvWKdbFn8+\nyfeyN3orSsin9QG0Yx3eqtO/1/6D5TtLsnY2/yPV/j0pv2GCCuB0kcKfygOQTYW6\nJrmYzeFeR/bnQM/lOM49leURdgC/x7tveNG7KRvD0X85M9iuT9/0+VSu6yAkcEi5\nx23C/Chzu7FFVxwZRHD+RshbV4QTPewhi17EJwroMYFpjGUHJVUfzo6W6bsWqA59\nCiiHI87NdBZv4JUCOQ==\n-----END CERTIFICATE-----\n";
const TEST_SERVER_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC/GfUhio45GzZi\nz/o67ngbdqW77Bk54aQ/L9kthS47SkqHVaC8+/AYXfDddl0Q+EHQleGrCqjPrRLA\nZAm0Qz0YZEdWcFX4F4T/gvVc7gfobPmkwoVoJwk/A7giuymN9+kpDbXsr2X29uMr\nfW0krA4ZpD/+JzLMXXfG5S9mazrs81wGqFlByGV32z3Qiecvd8JL4nnyz2mMTSqV\nOvtuh285dpgHFugOUGHhhXs9b/CPAGiBTBxuIm+2tGdjtmVGL8L9tlRMYnmQ7u9K\nUCAyHYovUPJjZazTqfkpQ2CkC4iOZ7Ay25gn0fbGBEysyOBZIPw39FEW63jGembu\nOVF6nO/DAgMBAAECggEAKLC7v80TVHiFX4veQZ8WRu7AAmAWzPrNMMEc8rLZcblz\nXhau956DdITILTevQFZEGUhYuUU3RaUaCYojgNUSVLfBctfPjlhfstItMYDjgSt3\nCox6wH8TWm4NzqNgiUCgzmODeaatROUz4MY/r5/NDsuo7pJlIBvEzb5uFdY+QUZ/\nR5gHRiD2Q3wCODe8zQRfTZGo7jCimAuWTLurWZl6ax/4TjWbXCD6DTuUo81cW3vy\nne6tEetHcABRO7uDoBYXk12pCgqFZzjLMnKJjQM+OYnSj6DoWjOu1drT5YyRLGDj\nfzN8V0aKRkOYoZ5QZOua8pByOyQElJnM16vkPtHgPQKBgQD6SOUNWEghvYIGM/lx\nc22/zjvDjeaGC3qSmlpQYN5MGuDoszeDBZ+rMTmHqJ9FcHYkLQnUI7ZkHhRGt/wQ\n/w3CroJjPBgKk+ipy2cBHSI+z+U20xjYzE8hxArWbXG1G4rDt5AIz68IQPsfkVND\nktkDABDaU+KwBPx8fjeeqtRQxQKBgQDDdxdLB1XcfZMX0KEP5RfA8ar1nW41TUAl\nTCOLaXIQbHZ0BeW7USE9mK8OKnVALZGJ+rpxvYFPZ5MWxchpb/cuIwXjLoN6uZVb\nfx4Hho+2iCfhcEKzs8XZW48duKIfhx13BiILLf/YaHAWFs9UfVcQog4Qx03guyMr\n7k9bFuy25wKBgQDpE48zAT6TJS775dTrAQp4b28aan/93pyz/8gRSFRb3UALlDIi\n8s7BluKzYaWI/fUXNVYM14EX9Sb+wIGdtlezL94+2Yyt9RXbYY8361Cj2+jiSG3A\nH2ulzzIkg+E7Pj3Yi443lmiysAjsWeKHcC5l697F4w6cytfye3wCZ6W23QKBgQC0\n9tX+5aytdSkwnDvxXlVOka+ItBcri/i+Ty59TMOIxxInuqoFcUhIIcq4X8CsCUQ8\nLYBd+2fznt3D8JrqWvnKoiw6N38MqTLJQfgIWaFGCep6QhfPDbo30RfAGYcnj01N\nO8Va+lxq+84B9V5AR8bKpG5HRG4qiLc4XerkV2YSswKBgDt9eerSBZyLVwfku25Y\nfrh+nEjUZy81LdlpJmu/bfa2FfItzBqDZPskkJJW9ON82z/ejGFbsU48RF7PJUMr\nGimE33QeTDToGozHCq0QOd0SMfsVkOQR+EROdmY52UIYAYgQUfI1FQ9lLsw10wlQ\nD11SHTL7b9pefBWfW73I7ttV\n-----END PRIVATE KEY-----\n";

#[tokio::test(flavor = "multi_thread")]
async fn upstream_server_name_override_sets_sni() {
    let (upstream_addr, observed_rx, upstream_task, upstream_temp_dir) =
        spawn_tls_upstream_capture_sni().await;

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-upstream-sni-override", |_| {
        proxy_config(listen_addr, upstream_addr, None, Some("localhost"))
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/",
        200,
        "upstream sni ok\n",
        Duration::from_secs(5),
    );

    let observed = tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .expect("upstream request should be observed before timeout")
        .expect("upstream observation should complete");
    assert_eq!(observed.as_deref(), Some("localhost"));

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn upstream_server_name_false_suppresses_sni() {
    let (upstream_addr, observed_rx, upstream_task, upstream_temp_dir) =
        spawn_tls_upstream_capture_sni().await;

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-upstream-sni-disabled", |_| {
        proxy_config(listen_addr, upstream_addr, Some(false), Some("localhost"))
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/",
        200,
        "upstream sni ok\n",
        Duration::from_secs(5),
    );

    let observed = tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .expect("upstream request should be observed before timeout")
        .expect("upstream observation should complete");
    assert_eq!(observed, None);

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
}

async fn spawn_tls_upstream_capture_sni(
) -> (SocketAddr, oneshot::Receiver<Option<String>>, JoinHandle<()>, PathBuf) {
    let temp_dir = temp_dir("rginx-upstream-sni-server");
    fs::create_dir_all(&temp_dir).expect("upstream temp dir should be created");
    let cert_path = temp_dir.join("upstream.crt");
    let key_path = temp_dir.join("upstream.key");
    fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("upstream cert should be written");
    fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("upstream key should be written");

    let certified_key = Arc::new(load_certified_key(&cert_path, &key_path));
    let (observed_tx, observed_rx) = oneshot::channel();
    let resolver = Arc::new(CapturingResolver {
        certified_key,
        observed_sni: Arc::new(Mutex::new(Some(observed_tx))),
    });
    let tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    let tls_acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("upstream TLS listener should bind");
    let listen_addr = listener.local_addr().expect("upstream TLS addr should be available");

    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("upstream TLS listener should accept");
        let tls_stream = tls_acceptor.accept(stream).await.expect("upstream TLS handshake should work");

        let service = service_fn(move |_request: Request<Incoming>| async move {
            Ok::<_, Infallible>(
                Response::builder()
                    .status(StatusCode::OK)
                    .header("content-type", "text/plain; charset=utf-8")
                    .body(Full::new(Bytes::from_static(b"upstream sni ok\n")))
                    .expect("upstream response should build"),
            )
        });

        http1::Builder::new()
            .serve_connection(TokioIo::new(tls_stream), service)
            .await
            .expect("upstream TLS connection should complete");
    });

    (listen_addr, observed_rx, task, temp_dir)
}

fn proxy_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    server_name: Option<bool>,
    server_name_override: Option<&str>,
) -> String {
    let server_name = server_name
        .map(|value| format!("            server_name: Some({value}),\n"))
        .unwrap_or_default();
    let server_name_override = server_name_override
        .map(|value| format!("            server_name_override: Some({value:?}),\n"))
        .unwrap_or_else(|| "            server_name_override: None,\n".to_string());

    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Auto,\n{server_name}{server_name_override}        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn load_certified_key(cert_path: &Path, key_path: &Path) -> CertifiedKey {
    let provider = aws_lc_rs::default_provider();
    CertifiedKey::from_der(load_certs(cert_path), load_private_key(key_path), &provider)
        .expect("test certified key should build")
}

fn load_certs(path: &Path) -> Vec<CertificateDer<'static>> {
    let file = File::open(path).expect("certificate file should open");
    let mut reader = BufReader::new(file);
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .expect("certificate PEM should parse")
}

fn load_private_key(path: &Path) -> PrivateKeyDer<'static> {
    let file = File::open(path).expect("private key file should open");
    let mut reader = BufReader::new(file);
    rustls_pemfile::private_key(&mut reader)
        .expect("private key PEM should parse")
        .expect("private key PEM should include at least one key")
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

struct CapturingResolver {
    certified_key: Arc<CertifiedKey>,
    observed_sni: Arc<Mutex<Option<oneshot::Sender<Option<String>>>>>,
}

impl fmt::Debug for CapturingResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapturingResolver").finish_non_exhaustive()
    }
}

impl ResolvesServerCert for CapturingResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        if let Some(sender) = self
            .observed_sni
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = sender.send(client_hello.server_name().map(str::to_string));
        }
        Some(self.certified_key.clone())
    }
}
