use std::env;
use std::io::{Read, Write};
use std::net::SocketAddr;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rcgen::{
    BasicConstraints, CertificateParams, CertifiedKey, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair,
};
use rginx_runtime::admin::{AdminRequest, AdminResponse, admin_socket_path_for_config};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn required_client_cert_rejects_anonymous_clients_and_accepts_authenticated_clients() {
    let fixture = TlsFixture::new("rginx-downstream-mtls-required");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-downstream-mtls-required",
        &fixture.server_cert_pem,
        &fixture.server_key_pem,
        |temp_dir, cert_path, key_path| {
            let ca_path = temp_dir.join("client-ca.pem");
            std::fs::write(&ca_path, &fixture.ca_cert_pem).expect("CA cert should be written");
            required_client_auth_config(listen_addr, cert_path, key_path, &ca_path)
        },
    );

    wait_for_https_text_response(
        &mut server,
        listen_addr,
        "localhost",
        "/",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
        200,
        "required mtls\n",
        Duration::from_secs(5),
    );

    let anonymous = fetch_https_text_response(listen_addr, "localhost", "/", None);
    assert!(anonymous.is_err(), "anonymous TLS client should be rejected");

    let authenticated = fetch_https_text_response(
        listen_addr,
        "localhost",
        "/",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
    )
    .expect("mTLS-authenticated client should succeed");
    assert_eq!(authenticated, (200, "required mtls\n".to_string()));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn optional_client_cert_allows_both_anonymous_and_authenticated_clients() {
    let fixture = TlsFixture::new("rginx-downstream-mtls-optional");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-downstream-mtls-optional",
        &fixture.server_cert_pem,
        &fixture.server_key_pem,
        |temp_dir, cert_path, key_path| {
            let ca_path = temp_dir.join("client-ca.pem");
            std::fs::write(&ca_path, &fixture.ca_cert_pem).expect("CA cert should be written");
            optional_client_auth_config(listen_addr, cert_path, key_path, &ca_path)
        },
    );

    wait_for_https_text_response(
        &mut server,
        listen_addr,
        "localhost",
        "/",
        None,
        200,
        "optional mtls\n",
        Duration::from_secs(5),
    );

    let anonymous =
        fetch_https_text_response(listen_addr, "localhost", "/", None).expect("anonymous client");
    assert_eq!(anonymous, (200, "optional mtls\n".to_string()));

    let authenticated = fetch_https_text_response(
        listen_addr,
        "localhost",
        "/",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
    )
    .expect("authenticated client");
    assert_eq!(authenticated, (200, "optional mtls\n".to_string()));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn required_mtls_updates_admin_status_and_counters() {
    let fixture = TlsFixture::new("rginx-downstream-mtls-admin");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-downstream-mtls-admin",
        &fixture.server_cert_pem,
        &fixture.server_key_pem,
        |temp_dir, cert_path, key_path| {
            let ca_path = temp_dir.join("client-ca.pem");
            std::fs::write(&ca_path, &fixture.ca_cert_pem).expect("CA cert should be written");
            required_client_auth_config(listen_addr, cert_path, key_path, &ca_path)
        },
    );

    wait_for_https_text_response(
        &mut server,
        listen_addr,
        "localhost",
        "/",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
        200,
        "required mtls\n",
        Duration::from_secs(5),
    );

    let anonymous = fetch_https_text_response(listen_addr, "localhost", "/", None);
    assert!(anonymous.is_err(), "anonymous TLS client should be rejected");

    let authenticated = fetch_https_text_response(
        listen_addr,
        "localhost",
        "/",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
    )
    .expect("authenticated client should succeed");
    assert_eq!(authenticated, (200, "required mtls\n".to_string()));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let status = match query_admin_socket(&socket_path, AdminRequest::GetStatus)
        .expect("admin status should succeed")
    {
        AdminResponse::Status(status) => status,
        other => panic!("unexpected admin response: {other:?}"),
    };
    assert_eq!(status.mtls.configured_listeners, 1);
    assert_eq!(status.mtls.required_listeners, 1);
    assert_eq!(status.mtls.optional_listeners, 0);
    assert!(status.mtls.authenticated_connections >= 1);
    assert!(status.mtls.authenticated_requests >= 1);
    assert!(status.mtls.handshake_failures_missing_client_cert >= 1);

    let counters = match query_admin_socket(&socket_path, AdminRequest::GetCounters)
        .expect("admin counters should succeed")
    {
        AdminResponse::Counters(counters) => counters,
        other => panic!("unexpected admin response: {other:?}"),
    };
    assert!(counters.downstream_mtls_authenticated_connections >= 1);
    assert!(counters.downstream_mtls_authenticated_requests >= 1);
    assert!(counters.downstream_tls_handshake_failures_missing_client_cert >= 1);

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn mtls_access_log_variables_render_client_identity() {
    let fixture = TlsFixture::new("rginx-downstream-mtls-access-log");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-downstream-mtls-access-log",
        &fixture.server_cert_pem,
        &fixture.server_key_pem,
        |temp_dir, cert_path, key_path| {
            let ca_path = temp_dir.join("client-ca.pem");
            std::fs::write(&ca_path, &fixture.ca_cert_pem).expect("CA cert should be written");
            format!(
                "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        access_log_format: Some(\"mtls=$tls_client_authenticated subject=\\\"$tls_client_subject\\\" san=\\\"$tls_client_san_dns_names\\\"\"),\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n            client_auth: Some(ServerClientAuthConfig(\n                mode: Optional,\n                ca_cert_path: {:?},\n            )),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"optional mtls\\n\"),\n            ),\n        ),\n    ],\n)\n",
                listen_addr.to_string(),
                cert_path.display().to_string(),
                key_path.display().to_string(),
                ca_path.display().to_string(),
                ready_route = READY_ROUTE_CONFIG,
            )
        },
    );

    wait_for_https_text_response(
        &mut server,
        listen_addr,
        "localhost",
        "/",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
        200,
        "optional mtls\n",
        Duration::from_secs(5),
    );
    let response = fetch_https_text_response(
        listen_addr,
        "localhost",
        "/",
        Some((&fixture.client_cert_path, &fixture.client_key_path)),
    )
    .expect("authenticated request should succeed");
    assert_eq!(response, (200, "optional mtls\n".to_string()));

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let output = server.combined_output();
        if output.contains("mtls=true")
            && output.contains("subject=\"CN=client.example.com\"")
            && output.contains("san=\"client.example.com\"")
        {
            break;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for mTLS access log line\n{}", output);
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    server.shutdown_and_wait(Duration::from_secs(5));
}

struct TlsFixture {
    _dir: PathBuf,
    ca_cert_pem: String,
    server_cert_pem: String,
    server_key_pem: String,
    client_cert_path: PathBuf,
    client_key_path: PathBuf,
}

impl TlsFixture {
    fn new(prefix: &str) -> Self {
        let dir = temp_dir(prefix);
        std::fs::create_dir_all(&dir).expect("fixture temp dir should be created");

        let ca = generate_ca();
        let server = generate_leaf_cert("localhost", &ca, ExtendedKeyUsagePurpose::ServerAuth);
        let client =
            generate_leaf_cert("client.example.com", &ca, ExtendedKeyUsagePurpose::ClientAuth);

        let client_cert_path = dir.join("client.crt");
        let client_key_path = dir.join("client.key");
        std::fs::write(&client_cert_path, client.cert.pem())
            .expect("client cert should be written");
        std::fs::write(&client_key_path, client.key_pair.serialize_pem())
            .expect("client key should be written");

        Self {
            _dir: dir,
            ca_cert_pem: ca.cert.pem(),
            server_cert_pem: server.cert.pem(),
            server_key_pem: server.key_pair.serialize_pem(),
            client_cert_path,
            client_key_path,
        }
    }
}

fn generate_ca() -> CertifiedKey {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name.push(DnType::CommonName, "rginx test ca");
    let key_pair = KeyPair::generate().expect("CA keypair should generate");
    let cert = params.self_signed(&key_pair).expect("CA cert should generate");
    CertifiedKey { cert, key_pair }
}

fn generate_leaf_cert(
    dns_name: &str,
    issuer: &CertifiedKey,
    usage: ExtendedKeyUsagePurpose,
) -> CertifiedKey {
    let mut params =
        CertificateParams::new(vec![dns_name.to_string()]).expect("leaf params should build");
    params.distinguished_name.push(DnType::CommonName, dns_name);
    params.extended_key_usages = vec![usage];
    let key_pair = KeyPair::generate().expect("leaf keypair should generate");
    let cert = params
        .signed_by(&key_pair, &issuer.cert, &issuer.key_pair)
        .expect("leaf cert should be signed");
    CertifiedKey { cert, key_pair }
}

fn wait_for_https_text_response(
    server: &mut ServerHarness,
    listen_addr: SocketAddr,
    host: &str,
    path: &str,
    client_identity: Option<(&Path, &Path)>,
    expected_status: u16,
    expected_body: &str,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    let mut last_error = String::new();

    while Instant::now() < deadline {
        server.assert_running();

        match fetch_https_text_response(listen_addr, host, path, client_identity) {
            Ok((status, body)) if status == expected_status && body == expected_body => return,
            Ok((status, body)) => {
                last_error = format!("unexpected response: status={status} body={body:?}");
            }
            Err(error) => last_error = error,
        }
    }

    panic!(
        "timed out waiting for expected HTTPS response on {listen_addr}{path}; last error: {}\n{}",
        last_error,
        server.combined_output()
    );
}

fn fetch_https_text_response(
    listen_addr: SocketAddr,
    host: &str,
    path: &str,
    client_identity: Option<(&Path, &Path)>,
) -> Result<(u16, String), String> {
    let tcp = std::net::TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    tcp.set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    tcp.set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;

    let builder = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(InsecureServerCertVerifier::new()));
    let config = match client_identity {
        Some((cert_path, key_path)) => {
            let certs = load_certs(cert_path)?;
            let key = load_private_key(key_path)?;
            builder
                .with_client_auth_cert(certs, key)
                .map_err(|error| format!("failed to configure client cert: {error}"))?
        }
        None => builder.with_no_client_auth(),
    };

    let server_name = ServerName::try_from("localhost".to_string())
        .map_err(|error| format!("invalid TLS server name: {error}"))?;
    let connection = ClientConnection::new(Arc::new(config), server_name)
        .map_err(|error| format!("failed to build TLS client: {error}"))?;
    let mut stream = StreamOwned::new(connection, tcp);

    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
        .map_err(|error| format!("failed to write HTTPS request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush HTTPS request: {error}"))?;

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

    Ok((status, body.to_string()))
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("failed to open cert `{}`: {error}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to parse cert `{}`: {error}", path.display()))
}

fn load_private_key(path: &Path) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("failed to open key `{}`: {error}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    rustls_pemfile::private_key(&mut reader)
        .map_err(|error| format!("failed to parse key `{}`: {error}", path.display()))?
        .ok_or_else(|| format!("no private key found in `{}`", path.display()))
}

fn required_client_auth_config(
    listen_addr: SocketAddr,
    cert_path: &Path,
    key_path: &Path,
    ca_path: &Path,
) -> String {
    common_client_auth_config(
        listen_addr,
        cert_path,
        key_path,
        ca_path,
        "Required",
        "required mtls\n",
    )
}

fn optional_client_auth_config(
    listen_addr: SocketAddr,
    cert_path: &Path,
    key_path: &Path,
    ca_path: &Path,
) -> String {
    common_client_auth_config(
        listen_addr,
        cert_path,
        key_path,
        ca_path,
        "Optional",
        "optional mtls\n",
    )
}

fn common_client_auth_config(
    listen_addr: SocketAddr,
    cert_path: &Path,
    key_path: &Path,
    ca_path: &Path,
    mode: &str,
    body: &str,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n            client_auth: Some(ServerClientAuthConfig(\n                mode: {},\n                ca_cert_path: {:?},\n            )),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({:?}),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        mode,
        ca_path.display().to_string(),
        body,
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn temp_dir(prefix: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!("{}/{}-{}-{}", env::temp_dir().display(), prefix, unique, id))
}

fn wait_for_admin_socket(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut last_error = String::new();

    while Instant::now() < deadline {
        if path.exists() {
            match query_admin_socket(path, AdminRequest::GetRevision) {
                Ok(_) => return,
                Err(error) => last_error = error,
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    panic!("timed out waiting for admin socket {}; last error: {}", path.display(), last_error);
}

fn query_admin_socket(path: &Path, request: AdminRequest) -> Result<AdminResponse, String> {
    let mut stream = UnixStream::connect(path).map_err(|error| {
        format!("failed to connect to admin socket {}: {error}", path.display())
    })?;
    let encoded = serde_json::to_vec(&request)
        .map_err(|error| format!("failed to encode admin request: {error}"))?;
    stream
        .write_all(&encoded)
        .map_err(|error| format!("failed to write admin request: {error}"))?;
    stream
        .write_all(b"\n")
        .map_err(|error| format!("failed to terminate admin request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush admin request: {error}"))?;

    let mut reader = std::io::BufReader::new(stream);
    let mut response = String::new();
    reader
        .read_to_string(&mut response)
        .map_err(|error| format!("failed to read admin response: {error}"))?;
    let line = response.lines().next().unwrap_or_default();
    serde_json::from_str(line).map_err(|error| format!("failed to decode admin response: {error}"))
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
