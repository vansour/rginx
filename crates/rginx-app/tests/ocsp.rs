#![cfg(unix)]

use std::env;
use std::fs;
use std::io::Write;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};
use rasn::types::{BitString, GeneralizedTime, Integer, ObjectIdentifier, OctetString};
use rasn_ocsp::{
    BasicOcspResponse as RasnBasicOcspResponse, CertId as RasnCertId, CertStatus as RasnCertStatus,
    OcspRequest as RasnOcspRequest, OcspResponse as RasnOcspResponse,
    OcspResponseStatus as RasnOcspResponseStatus, ResponderId as RasnResponderId,
    ResponseBytes as RasnResponseBytes, ResponseData as RasnResponseData,
    SingleResponse as RasnSingleResponse,
};
use rcgen::{
    BasicConstraints, CertificateParams, CustomExtension, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, PKCS_ED25519, PKCS_RSA_SHA256,
    SigningKey,
};
use sha1::Digest;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, read_http_head, reserve_loopback_addr};

#[test]
fn status_and_check_report_dynamic_ocsp_refresh_state() {
    let ocsp_requests = Arc::new(AtomicUsize::new(0));
    let ocsp_response_body = Arc::new(Mutex::new(Vec::new()));
    let responder_addr = spawn_ocsp_responder(ocsp_requests.clone(), ocsp_response_body.clone());
    let listen_addr = reserve_loopback_addr();

    let mut server = ServerHarness::spawn("rginx-ocsp-refresh", |temp_dir| {
        let cert_path = temp_dir.join("server-chain.pem");
        let key_path = temp_dir.join("server.key");
        let ocsp_path = temp_dir.join("server.ocsp");

        let ca = generate_ca_cert("rginx-ocsp-test-ca");
        let leaf = generate_leaf_cert_with_ocsp_aia(
            "localhost",
            &ca,
            &format!("http://127.0.0.1:{}/ocsp", responder_addr.port()),
        );
        fs::write(&cert_path, format!("{}{}", leaf.cert.pem(), ca.cert.pem()))
            .expect("certificate chain should be written");
        fs::write(&key_path, leaf.signing_key.serialize_pem())
            .expect("private key should be written");
        fs::write(&ocsp_path, b"").expect("empty OCSP cache file should be written");
        *ocsp_response_body.lock().expect("OCSP response body mutex should lock") =
            build_ocsp_response_for_certificate(&cert_path, &ca);

        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n            ocsp_staple_path: Some({:?}),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
            listen_addr.to_string(),
            cert_path.display().to_string(),
            key_path.display().to_string(),
            ocsp_path.display().to_string(),
            ready_route = READY_ROUTE_CONFIG,
        )
    });
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));
    let config_path = server.config_path().to_path_buf();

    let status_stdout =
        wait_for_command_output(&config_path, &["status"], Duration::from_secs(10), |stdout| {
            stdout.contains("kind=status_tls_ocsp scope=listener:default")
                && stdout.contains("auto_refresh_enabled=true")
                && stdout.contains("cache_loaded=true")
                && stdout.contains("refreshes_total=")
        });
    assert!(status_stdout.contains("responder_urls=http://127.0.0.1:"));
    assert!(status_stdout.contains("staple_path="));
    assert!(status_stdout.contains("last_error=-"));
    assert!(ocsp_requests.load(Ordering::Relaxed) >= 1);

    let check_output = run_rginx_with_config(&config_path, &["check"]);
    assert!(
        check_output.status.success(),
        "check should succeed: {}",
        render_output(&check_output)
    );
    let check_stdout = String::from_utf8_lossy(&check_output.stdout);
    assert!(check_stdout.contains("tls_ocsp scope=listener:default"));
    assert!(check_stdout.contains("auto_refresh_enabled=true"));
    assert!(check_stdout.contains("cache_loaded=true"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn invalid_dynamic_ocsp_response_is_rejected_before_cache_write() {
    let ocsp_requests = Arc::new(AtomicUsize::new(0));
    let ocsp_response_body = Arc::new(Mutex::new(b"dummy-ocsp-response".to_vec()));
    let responder_addr = spawn_ocsp_responder(ocsp_requests.clone(), ocsp_response_body);
    let listen_addr = reserve_loopback_addr();

    let mut server = ServerHarness::spawn("rginx-ocsp-invalid-refresh", |temp_dir| {
        let cert_path = temp_dir.join("server-chain.pem");
        let key_path = temp_dir.join("server.key");
        let ocsp_path = temp_dir.join("server.ocsp");

        let ca = generate_ca_cert("rginx-ocsp-test-ca");
        let leaf = generate_leaf_cert_with_ocsp_aia(
            "localhost",
            &ca,
            &format!("http://127.0.0.1:{}/ocsp", responder_addr.port()),
        );
        fs::write(&cert_path, format!("{}{}", leaf.cert.pem(), ca.cert.pem()))
            .expect("certificate chain should be written");
        fs::write(&key_path, leaf.signing_key.serialize_pem())
            .expect("private key should be written");
        fs::write(&ocsp_path, b"").expect("empty OCSP cache file should be written");

        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n            ocsp_staple_path: Some({:?}),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
            listen_addr.to_string(),
            cert_path.display().to_string(),
            key_path.display().to_string(),
            ocsp_path.display().to_string(),
            ready_route = READY_ROUTE_CONFIG,
        )
    });
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));
    let config_path = server.config_path().to_path_buf();

    let status_stdout =
        wait_for_command_output(&config_path, &["status"], Duration::from_secs(10), |stdout| {
            stdout.contains("kind=status_tls_ocsp scope=listener:default")
                && stdout.contains("cache_loaded=false")
                && stdout.contains("failures_total=")
                && !stdout.contains("last_error=-")
        });
    assert!(status_stdout.contains("failed to parse OCSP response"));
    assert!(ocsp_requests.load(Ordering::Relaxed) >= 1);

    let cache =
        fs::read(config_path.parent().expect("config path should have parent").join("server.ocsp"))
            .expect("OCSP cache file should be readable");
    assert!(cache.is_empty(), "invalid OCSP response should not be cached");

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn expired_ocsp_cache_is_cleared_when_refresh_fails() {
    let ocsp_requests = Arc::new(AtomicUsize::new(0));
    let ocsp_response_body = Arc::new(Mutex::new(b"dummy-ocsp-response".to_vec()));
    let responder_addr = spawn_ocsp_responder(ocsp_requests.clone(), ocsp_response_body);
    let listen_addr = reserve_loopback_addr();

    let mut server = ServerHarness::spawn("rginx-ocsp-expired-cache", |temp_dir| {
        let cert_path = temp_dir.join("server-chain.pem");
        let key_path = temp_dir.join("server.key");
        let ocsp_path = temp_dir.join("server.ocsp");

        let ca = generate_ca_cert("rginx-ocsp-test-ca");
        let leaf = generate_leaf_cert_with_ocsp_aia(
            "localhost",
            &ca,
            &format!("http://127.0.0.1:{}/ocsp", responder_addr.port()),
        );
        fs::write(&cert_path, format!("{}{}", leaf.cert.pem(), ca.cert.pem()))
            .expect("certificate chain should be written");
        fs::write(&key_path, leaf.signing_key.serialize_pem())
            .expect("private key should be written");
        fs::write(
            &ocsp_path,
            build_ocsp_response_for_certificate_with_offsets(
                &cert_path,
                &ca,
                TimeOffset::Before(Duration::from_secs(2 * 24 * 60 * 60)),
                TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
            ),
        )
        .expect("expired OCSP cache file should be written");

        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n            ocsp_staple_path: Some({:?}),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
            listen_addr.to_string(),
            cert_path.display().to_string(),
            key_path.display().to_string(),
            ocsp_path.display().to_string(),
            ready_route = READY_ROUTE_CONFIG,
        )
    });
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));
    let config_path = server.config_path().to_path_buf();
    let ocsp_path =
        config_path.parent().expect("config path should have parent").join("server.ocsp");

    let status_stdout =
        wait_for_command_output(&config_path, &["status"], Duration::from_secs(10), |stdout| {
            stdout.contains("kind=status_tls_ocsp scope=listener:default")
                && stdout.contains("cache_loaded=false")
                && stdout.contains("cache_size_bytes=0")
        });
    assert!(
        status_stdout.contains("failed to parse OCSP response")
            || status_stdout.contains("is expired")
    );
    let cache = fs::read(&ocsp_path).expect("OCSP cache file should be readable");
    assert!(cache.is_empty(), "expired OCSP cache should be cleared after refresh failure");
    assert!(ocsp_requests.load(Ordering::Relaxed) >= 1);

    server.shutdown_and_wait(Duration::from_secs(5));
}

fn spawn_ocsp_responder(requests: Arc<AtomicUsize>, body: Arc<Mutex<Vec<u8>>>) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("OCSP responder should bind");
    let listen_addr = listener.local_addr().expect("OCSP responder addr should be available");

    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let _ = read_http_head(&mut stream);
            requests.fetch_add(1, Ordering::Relaxed);
            let body = body.lock().expect("OCSP response body mutex should lock").clone();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/ocsp-response\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });

    listen_addr
}

fn wait_for_command_output(
    config_path: &Path,
    args: &[&str],
    timeout: Duration,
    predicate: impl Fn(&str) -> bool,
) -> String {
    let deadline = Instant::now() + timeout;
    let mut last_stdout = String::new();
    let mut last_stderr = String::new();

    while Instant::now() < deadline {
        let output = run_rginx_with_config(config_path, args);
        last_stdout = String::from_utf8_lossy(&output.stdout).to_string();
        last_stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if output.status.success() && predicate(&last_stdout) {
            return last_stdout;
        }
        thread::sleep(Duration::from_millis(100));
    }

    panic!("timed out waiting for command output; stdout={last_stdout:?}; stderr={last_stderr:?}");
}

fn run_rginx_with_config(config_path: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(binary_path());
    command.arg("--config").arg(config_path);
    command.args(args);
    command.output().expect("rginx command should run")
}

fn binary_path() -> PathBuf {
    env::var_os("CARGO_BIN_EXE_rginx")
        .map(PathBuf::from)
        .expect("cargo should expose the rginx test binary path")
}

fn render_output(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
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
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let signing_key = KeyPair::generate().expect("CA keypair should generate");
    let cert = params.self_signed(&signing_key).expect("CA certificate should self-sign");
    TestCertifiedKey { cert, signing_key, params }
}

fn generate_leaf_cert_with_ocsp_aia(
    dns_name: &str,
    issuer: &TestCertifiedKey,
    responder_url: &str,
) -> TestCertifiedKey {
    let mut params =
        CertificateParams::new(vec![dns_name.to_string()]).expect("leaf params should build");
    params.distinguished_name.push(DnType::CommonName, dns_name);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.custom_extensions.push(CustomExtension::from_oid_content(
        &[1, 3, 6, 1, 5, 5, 7, 1, 1],
        authority_info_access_extension_value(responder_url),
    ));
    let signing_key = KeyPair::generate().expect("leaf keypair should generate");
    let cert = params
        .signed_by(&signing_key, &issuer.issuer())
        .expect("leaf certificate should be signed");
    TestCertifiedKey { cert, signing_key, params }
}

fn authority_info_access_extension_value(responder_url: &str) -> Vec<u8> {
    der_sequence([der_sequence([
        vec![0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01],
        der_context_6_ia5_string(responder_url.as_bytes()),
    ])])
}

fn der_sequence<const N: usize>(elements: [Vec<u8>; N]) -> Vec<u8> {
    let payload = elements.into_iter().flatten().collect::<Vec<_>>();
    der_wrap(0x30, payload)
}

fn der_context_6_ia5_string(bytes: &[u8]) -> Vec<u8> {
    der_wrap(0x86, bytes.to_vec())
}

fn der_wrap(tag: u8, payload: Vec<u8>) -> Vec<u8> {
    let mut encoded = Vec::new();
    encoded.push(tag);
    encoded.extend(der_length(payload.len()));
    encoded.extend(payload);
    encoded
}

fn der_length(length: usize) -> Vec<u8> {
    if length < 0x80 {
        return vec![length as u8];
    }
    let bytes = length.to_be_bytes().into_iter().skip_while(|byte| *byte == 0).collect::<Vec<_>>();
    let mut encoded = Vec::with_capacity(bytes.len() + 1);
    encoded.push(0x80 | (bytes.len() as u8));
    encoded.extend(bytes);
    encoded
}

fn build_ocsp_response_for_certificate(cert_path: &Path, issuer: &TestCertifiedKey) -> Vec<u8> {
    build_ocsp_response_for_certificate_with_offsets(
        cert_path,
        issuer,
        TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
        TimeOffset::After(Duration::from_secs(24 * 60 * 60)),
    )
}

fn build_ocsp_response_for_certificate_with_offsets(
    cert_path: &Path,
    issuer: &TestCertifiedKey,
    this_update_offset: TimeOffset,
    next_update_offset: TimeOffset,
) -> Vec<u8> {
    let request = rginx_http::build_ocsp_request_for_certificate(cert_path)
        .expect("OCSP request should build for certificate chain");
    let cert_id = extract_ocsp_cert_id_from_request(&request);
    let now = SystemTime::now();
    let this_update = ocsp_time_with_offset(now, this_update_offset);
    let next_update = ocsp_time_with_offset(now, next_update_offset);
    let tbs_response_data = RasnResponseData {
        version: Integer::from(0),
        responder_id: responder_id_for_certificate(issuer.cert.der().as_ref()),
        produced_at: this_update,
        responses: vec![RasnSingleResponse {
            cert_id,
            cert_status: RasnCertStatus::Good,
            this_update,
            next_update: Some(next_update),
            single_extensions: None,
        }],
        response_extensions: None,
    };
    let tbs_der =
        rasn::der::encode(&tbs_response_data).expect("response data should encode for signing");
    let signature = issuer.signing_key.sign(&tbs_der).expect("OCSP response should sign");
    let basic = RasnBasicOcspResponse {
        tbs_response_data,
        signature_algorithm: test_signature_algorithm(&issuer.signing_key),
        signature: BitString::from_slice(&signature),
        certs: None,
    };
    let basic_der = rasn::der::encode(&basic).expect("basic OCSP response should encode");
    rasn::der::encode(&RasnOcspResponse {
        status: RasnOcspResponseStatus::Successful,
        bytes: Some(RasnResponseBytes {
            r#type: basic_ocsp_response_type_oid(),
            response: OctetString::from_slice(&basic_der),
        }),
    })
    .expect("OCSP response should encode")
}

fn extract_ocsp_cert_id_from_request(request_der: &[u8]) -> RasnCertId {
    let request: RasnOcspRequest =
        rasn::der::decode(request_der).expect("OCSP request should decode");
    request
        .tbs_request
        .request_list
        .first()
        .map(|request| request.req_cert.clone())
        .expect("OCSP request should contain a CertId")
}

enum TimeOffset {
    Before(Duration),
    After(Duration),
}

fn ocsp_time_with_offset(base: SystemTime, offset: TimeOffset) -> GeneralizedTime {
    let time = match offset {
        TimeOffset::Before(duration) => {
            base.checked_sub(duration).expect("time offset should stay after unix epoch")
        }
        TimeOffset::After(duration) => base + duration,
    };
    generalized_time_from_system_time(time)
}

fn basic_ocsp_response_type_oid() -> ObjectIdentifier {
    ObjectIdentifier::new(vec![1, 3, 6, 1, 5, 5, 7, 48, 1, 1])
        .expect("basic OCSP response type OID should be valid")
}

fn generalized_time_from_system_time(time: SystemTime) -> GeneralizedTime {
    let utc = DateTime::<Utc>::from(time);
    utc.fixed_offset()
}

fn responder_id_for_certificate(cert_der: &[u8]) -> RasnResponderId {
    let cert: rasn_pkix::Certificate = rasn::der::decode(cert_der).expect("certificate should decode");
    RasnResponderId::ByKey(OctetString::from(
        sha1::Sha1::digest(cert.tbs_certificate.subject_public_key_info.subject_public_key.as_raw_slice())
            .to_vec(),
    ))
}

fn test_signature_algorithm(key: &KeyPair) -> rasn_pkix::AlgorithmIdentifier {
    let der = if key.algorithm() == &PKCS_ECDSA_P256_SHA256 {
        &[0x30, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02][..]
    } else if key.algorithm() == &PKCS_RSA_SHA256 {
        &[0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b, 0x05, 0x00]
            [..]
    } else if key.algorithm() == &PKCS_ED25519 {
        &[0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70][..]
    } else {
        panic!("unsupported OCSP test signature algorithm");
    };
    rasn::der::decode(der).expect("signature algorithm should decode")
}
