use std::env;
use std::fmt;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use h3::server::Connection as H3Connection;
use h3_quinn::quinn;
use hyper::{Response, StatusCode};
use quinn::crypto::rustls::QuicServerConfig;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};
use rustls::crypto::aws_lc_rs;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use rustls::{
    DigitallySignedStruct, DistinguishedName, ProtocolVersion, RootCertStore, SignatureScheme,
};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedHttp3Request {
    sni: Option<String>,
    peer_certificates_present: bool,
    protocol_version: Option<ProtocolVersion>,
    path: String,
}

#[tokio::test(flavor = "multi_thread")]
async fn proxies_plain_http_requests_to_http3_upstreams() {
    let cert = generate_cert("localhost");
    let shared_dir = temp_dir("rginx-upstream-h3-basic");
    fs::create_dir_all(&shared_dir).expect("shared temp dir should be created");
    let server_cert_path = shared_dir.join("server.pem");
    let server_key_path = shared_dir.join("server.key");
    fs::write(&server_cert_path, cert.cert.pem()).expect("server cert should be written");
    fs::write(&server_key_path, cert.signing_key.serialize_pem())
        .expect("server key should be written");

    let (upstream_addr, observed_rx, upstream_task, upstream_temp_dir) =
        spawn_http3_upstream(&server_cert_path, &server_key_path, None, false).await;

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-upstream-http3-basic", |_| {
        basic_proxy_config(listen_addr, upstream_addr)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/",
        200,
        "upstream http3 ok\n",
        Duration::from_secs(5),
    );

    let observed = tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .expect("upstream request should be observed before timeout")
        .expect("upstream observation channel should complete");
    assert_eq!(observed.path, "/");
    assert_eq!(observed.sni, None);
    assert!(!observed.peer_certificates_present);
    assert_eq!(observed.protocol_version, Some(ProtocolVersion::TLSv1_3));

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream h3 task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
    fs::remove_dir_all(shared_dir).expect("shared temp dir should be removed");
}

#[tokio::test(flavor = "multi_thread")]
async fn upstream_http3_honors_server_name_override_and_client_identity() {
    let shared_dir = temp_dir("rginx-upstream-h3-mtls");
    fs::create_dir_all(&shared_dir).expect("shared temp dir should be created");

    let ca = generate_ca_cert("upstream-h3-ca");
    let server = generate_cert_signed_by_ca("localhost", &ca);
    let client = generate_cert_signed_by_ca("upstream-h3-client", &ca);

    let ca_path = shared_dir.join("ca.pem");
    let server_cert_path = shared_dir.join("server.crt");
    let server_key_path = shared_dir.join("server.key");
    let client_cert_path = shared_dir.join("client.crt");
    let client_key_path = shared_dir.join("client.key");

    fs::write(&ca_path, ca.cert.pem()).expect("ca cert should be written");
    fs::write(&server_cert_path, server.cert.pem()).expect("server cert should be written");
    fs::write(&server_key_path, server.signing_key.serialize_pem())
        .expect("server key should be written");
    fs::write(&client_cert_path, client.cert.pem()).expect("client cert should be written");
    fs::write(&client_key_path, client.signing_key.serialize_pem())
        .expect("client key should be written");

    let (upstream_addr, observed_rx, upstream_task, upstream_temp_dir) =
        spawn_http3_upstream(&server_cert_path, &server_key_path, Some(&ca_path), true).await;

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-upstream-http3-mtls", |_| {
        mtls_proxy_config(listen_addr, upstream_addr, &ca_path, &client_cert_path, &client_key_path)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    server.wait_for_http_text_response(
        listen_addr,
        &listen_addr.to_string(),
        "/",
        200,
        "upstream http3 ok\n",
        Duration::from_secs(5),
    );

    let observed = tokio::time::timeout(Duration::from_secs(5), observed_rx)
        .await
        .expect("upstream request should be observed before timeout")
        .expect("upstream observation channel should complete");
    assert_eq!(observed.path, "/");
    assert_eq!(observed.sni.as_deref(), Some("localhost"));
    assert!(observed.peer_certificates_present);
    assert_eq!(observed.protocol_version, Some(ProtocolVersion::TLSv1_3));

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.await.expect("upstream h3 task should finish");
    fs::remove_dir_all(upstream_temp_dir).expect("upstream temp dir should be removed");
    fs::remove_dir_all(shared_dir).expect("shared temp dir should be removed");
}

async fn spawn_http3_upstream(
    cert_path: &Path,
    key_path: &Path,
    client_ca_path: Option<&Path>,
    require_client_cert: bool,
) -> (SocketAddr, oneshot::Receiver<ObservedHttp3Request>, JoinHandle<()>, PathBuf) {
    let temp_dir = temp_dir("rginx-upstream-http3-server");
    fs::create_dir_all(&temp_dir).expect("upstream temp dir should be created");

    let certified_key = Arc::new(load_certified_key(cert_path, key_path));
    let observed_sni = Arc::new(Mutex::new(None::<String>));
    let resolver =
        Arc::new(CapturingResolver { certified_key, observed_sni: observed_sni.clone() });
    let client_cert_seen = Arc::new(AtomicBool::new(false));

    let builder = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .expect("server TLS1.3 builder should succeed");
    let mut server_crypto = if require_client_cert {
        let verifier = Arc::new(RequireTrustedClientCertVerifier::new(
            client_ca_path.expect("client CA should be provided when client certs are required"),
            client_cert_seen.clone(),
        ));
        builder.with_client_cert_verifier(verifier).with_cert_resolver(resolver)
    } else {
        builder.with_no_client_auth().with_cert_resolver(resolver)
    };
    server_crypto.alpn_protocols = vec![b"h3".to_vec()];

    let server_config = quinn::ServerConfig::with_crypto(Arc::new(
        QuicServerConfig::try_from(server_crypto).expect("quic server config should build"),
    ));
    let endpoint = quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap()).unwrap();
    let listen_addr = endpoint.local_addr().expect("upstream h3 addr should be available");
    let (observed_tx, observed_rx) = oneshot::channel();

    let task = tokio::spawn(async move {
        let incoming = endpoint.accept().await.expect("upstream h3 connection should arrive");
        let connection = incoming.await.expect("upstream h3 connection should establish");
        let protocol_version = Some(ProtocolVersion::TLSv1_3);
        let mut h3 = H3Connection::new(h3_quinn::Connection::new(connection))
            .await
            .expect("upstream h3 server should initialize");
        let resolver = h3
            .accept()
            .await
            .expect("upstream h3 should accept request")
            .expect("request should exist");
        let (request, mut stream) =
            resolver.resolve_request().await.expect("upstream h3 should resolve request");
        let observed = ObservedHttp3Request {
            sni: observed_sni.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).clone(),
            peer_certificates_present: client_cert_seen.load(Ordering::Relaxed),
            protocol_version,
            path: request.uri().path().to_string(),
        };
        let _ = observed_tx.send(observed);

        stream
            .send_response(
                Response::builder().status(StatusCode::OK).body(()).expect("response should build"),
            )
            .await
            .expect("upstream h3 should send response");
        stream
            .send_data(Bytes::from_static(b"upstream http3 ok\n"))
            .await
            .expect("upstream h3 should send body");
        stream.finish().await.expect("upstream h3 should finish response");
        tokio::time::sleep(Duration::from_millis(50)).await;
    });

    (listen_addr, observed_rx, task, temp_dir)
}

fn basic_proxy_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http3,\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn mtls_proxy_config(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    ca_path: &Path,
    client_cert_path: &Path,
    client_key_path: &Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(UpstreamTlsConfig(\n                verify: CustomCa(ca_cert_path: {:?}),\n                versions: Some([Tls13]),\n                client_cert_path: Some({:?}),\n                client_key_path: Some({:?}),\n            )),\n            protocol: Http3,\n            server_name_override: Some(\"localhost\"),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        ca_path.display().to_string(),
        client_cert_path.display().to_string(),
        client_key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn load_certified_key(cert_path: &Path, key_path: &Path) -> CertifiedKey {
    let provider = aws_lc_rs::default_provider();
    CertifiedKey::from_der(load_certs(cert_path), load_private_key(key_path), &provider)
        .expect("test certified key should build")
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

struct CapturingResolver {
    certified_key: Arc<CertifiedKey>,
    observed_sni: Arc<Mutex<Option<String>>>,
}

impl fmt::Debug for CapturingResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CapturingResolver").finish_non_exhaustive()
    }
}

impl ResolvesServerCert for CapturingResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        if let Some(server_name) = client_hello.server_name() {
            *self.observed_sni.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) =
                Some(server_name.to_string());
        }
        Some(self.certified_key.clone())
    }
}

#[derive(Debug)]
struct RequireTrustedClientCertVerifier {
    inner: Arc<dyn ClientCertVerifier>,
    client_cert_seen: Arc<AtomicBool>,
}

impl RequireTrustedClientCertVerifier {
    fn new(ca_path: &Path, client_cert_seen: Arc<AtomicBool>) -> Self {
        let mut roots = RootCertStore::empty();
        for cert in load_certs(ca_path) {
            roots.add(cert).expect("client CA certificate should load into root store");
        }
        let inner = WebPkiClientVerifier::builder(roots.into())
            .build()
            .expect("client verifier should build");
        Self { inner, client_cert_seen }
    }
}

impl ClientCertVerifier for RequireTrustedClientCertVerifier {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        self.inner.root_hint_subjects()
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: rustls::pki_types::UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        let verified = self.inner.verify_client_cert(end_entity, intermediates, now)?;
        self.client_cert_seen.store(true, Ordering::Relaxed);
        Ok(verified)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls12_signature(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        self.inner.verify_tls13_signature(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.inner.supported_verify_schemes()
    }
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
    let mut params =
        CertificateParams::new(vec![common_name.to_string()]).expect("CA params should build");
    params.distinguished_name.push(DnType::CommonName, common_name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let signing_key = KeyPair::generate().expect("CA keypair should generate");
    let cert = params.self_signed(&signing_key).expect("CA certificate should self-sign");
    TestCertifiedKey { cert, signing_key, params }
}

fn generate_cert(hostname: &str) -> TestCertifiedKey {
    let params =
        CertificateParams::new(vec![hostname.to_string()]).expect("leaf params should build");
    let signing_key = KeyPair::generate().expect("leaf key should generate");
    let cert = params.self_signed(&signing_key).expect("self-signed cert should generate");
    TestCertifiedKey { cert, signing_key, params }
}

fn generate_cert_signed_by_ca(hostname: &str, ca: &TestCertifiedKey) -> TestCertifiedKey {
    let mut params =
        CertificateParams::new(vec![hostname.to_string()]).expect("leaf params should build");
    params.distinguished_name.push(DnType::CommonName, hostname);
    let signing_key = KeyPair::generate().expect("leaf key should generate");
    let cert =
        params.signed_by(&signing_key, &ca.issuer()).expect("leaf cert should be signed by ca");
    TestCertifiedKey { cert, signing_key, params }
}
