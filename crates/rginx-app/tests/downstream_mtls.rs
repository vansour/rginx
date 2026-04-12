#[allow(unused_imports)]
use std::env;
#[allow(unused_imports)]
use std::io::{Read, Write};
#[allow(unused_imports)]
use std::net::SocketAddr;
#[allow(unused_imports)]
use std::os::unix::net::UnixStream;
#[allow(unused_imports)]
use std::path::{Path, PathBuf};
#[allow(unused_imports)]
use std::sync::Arc;
#[allow(unused_imports)]
use std::sync::atomic::{AtomicU64, Ordering};
#[allow(unused_imports)]
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[allow(unused_imports)]
use rcgen::{
    BasicConstraints, CertificateParams, CertificateRevocationList,
    CertificateRevocationListParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyIdMethod,
    KeyPair, KeyUsagePurpose, RevocationReason, RevokedCertParams, SerialNumber, date_time_ymd,
};
#[allow(unused_imports)]
use rginx_runtime::admin::{AdminRequest, AdminResponse, admin_socket_path_for_config};
#[allow(unused_imports)]
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
#[allow(unused_imports)]
use rustls::pki_types::pem::PemObject;
#[allow(unused_imports)]
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
#[allow(unused_imports)]
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[path = "downstream_mtls/enforcement.rs"]
mod enforcement;
#[path = "downstream_mtls/observability.rs"]
mod observability;
#[path = "downstream_mtls/validation.rs"]
mod validation;

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
        std::fs::write(&client_key_path, client.signing_key.serialize_pem())
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

fn generate_ca() -> TestCertifiedKey {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name.push(DnType::CommonName, "rginx test ca");
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::CrlSign,
    ];
    let signing_key = KeyPair::generate().expect("CA keypair should generate");
    let cert = params.self_signed(&signing_key).expect("CA cert should generate");
    TestCertifiedKey { cert, signing_key, params }
}

fn generate_intermediate_ca(common_name: &str, issuer: &TestCertifiedKey) -> TestCertifiedKey {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.distinguished_name.push(DnType::CommonName, common_name);
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::CrlSign,
    ];
    let signing_key = KeyPair::generate().expect("intermediate keypair should generate");
    let cert = params
        .signed_by(&signing_key, &issuer.issuer())
        .expect("intermediate cert should be signed");
    TestCertifiedKey { cert, signing_key, params }
}

fn generate_leaf_cert(
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

fn generate_leaf_cert_with_serial(
    dns_name: &str,
    issuer: &TestCertifiedKey,
    usage: ExtendedKeyUsagePurpose,
    serial: u64,
) -> TestCertifiedKey {
    let mut params =
        CertificateParams::new(vec![dns_name.to_string()]).expect("leaf params should build");
    params.distinguished_name.push(DnType::CommonName, dns_name);
    params.extended_key_usages = vec![usage];
    params.serial_number = Some(SerialNumber::from(serial));
    let signing_key = KeyPair::generate().expect("leaf keypair should generate");
    let cert =
        params.signed_by(&signing_key, &issuer.issuer()).expect("leaf cert should be signed");
    TestCertifiedKey { cert, signing_key, params }
}

fn generate_client_auth_crl(
    issuer: &TestCertifiedKey,
    revoked_serial: u64,
) -> CertificateRevocationList {
    CertificateRevocationListParams {
        this_update: date_time_ymd(2024, 1, 1),
        next_update: date_time_ymd(2027, 1, 1),
        crl_number: SerialNumber::from(1),
        issuing_distribution_point: None,
        revoked_certs: vec![RevokedCertParams {
            serial_number: SerialNumber::from(revoked_serial),
            revocation_time: date_time_ymd(2024, 1, 2),
            reason_code: Some(RevocationReason::KeyCompromise),
            invalidity_date: None,
        }],
        key_identifier_method: KeyIdMethod::Sha256,
    }
    .signed_by(&issuer.issuer())
    .expect("CRL should be signed")
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
    CertificateDer::pem_file_iter(path)
        .map_err(|error| format!("failed to open cert `{}`: {error}", path.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to parse cert `{}`: {error}", path.display()))
}

fn load_private_key(path: &Path) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    rustls::pki_types::PrivateKeyDer::from_pem_file(path)
        .map_err(|error| format!("failed to parse key `{}`: {error}", path.display()))
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
    common_client_auth_config_with_extra(listen_addr, cert_path, key_path, ca_path, mode, body, "")
}

fn common_client_auth_config_with_extra(
    listen_addr: SocketAddr,
    cert_path: &Path,
    key_path: &Path,
    ca_path: &Path,
    mode: &str,
    body: &str,
    client_auth_extra: &str,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n            client_auth: Some(ServerClientAuthConfig(\n                mode: {},\n                ca_cert_path: {:?},\n{client_auth_extra}            )),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({:?}),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        mode,
        ca_path.display().to_string(),
        body,
        client_auth_extra = client_auth_extra,
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
