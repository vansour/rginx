#![cfg(unix)]

use std::env;
use std::fs;
use std::io::Write;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use rcgen::{
    BasicConstraints, CertificateParams, CertifiedKey, CustomExtension, DnType,
    ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose,
};

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, read_http_head, reserve_loopback_addr};

#[test]
fn status_and_check_report_dynamic_ocsp_refresh_state() {
    let ocsp_requests = Arc::new(AtomicUsize::new(0));
    let responder_addr = spawn_ocsp_responder(ocsp_requests.clone(), b"dummy-ocsp-response");
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
        fs::write(&key_path, leaf.key_pair.serialize_pem()).expect("private key should be written");
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

fn spawn_ocsp_responder(requests: Arc<AtomicUsize>, body: &'static [u8]) -> SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("OCSP responder should bind");
    let listen_addr = listener.local_addr().expect("OCSP responder addr should be available");

    thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let _ = read_http_head(&mut stream);
            requests.fetch_add(1, Ordering::Relaxed);
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/ocsp-response\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.write_all(body);
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

fn generate_ca_cert(common_name: &str) -> CertifiedKey {
    let mut params =
        CertificateParams::new(vec![common_name.to_string()]).expect("CA params should build");
    params.distinguished_name.push(DnType::CommonName, common_name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let key_pair = KeyPair::generate().expect("CA keypair should generate");
    let cert = params.self_signed(&key_pair).expect("CA certificate should self-sign");
    CertifiedKey { cert, key_pair }
}

fn generate_leaf_cert_with_ocsp_aia(
    dns_name: &str,
    issuer: &CertifiedKey,
    responder_url: &str,
) -> CertifiedKey {
    let mut params =
        CertificateParams::new(vec![dns_name.to_string()]).expect("leaf params should build");
    params.distinguished_name.push(DnType::CommonName, dns_name);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.custom_extensions.push(CustomExtension::from_oid_content(
        &[1, 3, 6, 1, 5, 5, 7, 1, 1],
        authority_info_access_extension_value(responder_url),
    ));
    let key_pair = KeyPair::generate().expect("leaf keypair should generate");
    let cert = params
        .signed_by(&key_pair, &issuer.cert, &issuer.key_pair)
        .expect("leaf certificate should be signed");
    CertifiedKey { cert, key_pair }
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
