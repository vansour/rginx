#![cfg(unix)]
#![allow(unused_imports)]

use std::io::{BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use rcgen::{
    BasicConstraints, CertificateParams, CertificateRevocationList,
    CertificateRevocationListParams, DnType, IsCa, Issuer, KeyIdMethod, KeyPair, KeyUsagePurpose,
    RevocationReason, RevokedCertParams, SerialNumber, date_time_ymd,
};

mod support;

use rginx_runtime::admin::{
    AdminRequest, AdminResponse, RevisionSnapshot, admin_socket_path_for_config,
};
use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[path = "admin/commands.rs"]
mod commands;
#[path = "admin/delta_wait.rs"]
mod delta_wait;
#[path = "admin/snapshot.rs"]
mod snapshot;

fn wait_for_admin_socket(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut last_error = String::new();

    while Instant::now() < deadline {
        if path.exists() {
            match query_admin_socket(path, AdminRequest::GetRevision) {
                Ok(_) => return,
                Err(error) => last_error = error.to_string(),
            }
        } else {
            last_error = format!("socket {} does not exist yet", path.display());
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    panic!("timed out waiting for admin socket {}; last error: {}", path.display(), last_error);
}

fn query_admin_socket(path: &Path, request: AdminRequest) -> Result<AdminResponse, String> {
    let mut stream = UnixStream::connect(path)
        .map_err(|error| format!("failed to connect to {}: {error}", path.display()))?;
    serde_json::to_writer(&mut stream, &request)
        .map_err(|error| format!("failed to encode request: {error}"))?;
    stream.write_all(b"\n").map_err(|error| format!("failed to terminate request: {error}"))?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(|error| format!("failed to shutdown write side: {error}"))?;

    let mut response = String::new();
    BufReader::new(stream)
        .read_to_string(&mut response)
        .map_err(|error| format!("failed to read response: {error}"))?;
    serde_json::from_str(response.trim())
        .map_err(|error| format!("failed to decode response: {error}"))
}

fn run_rginx(args: impl IntoIterator<Item = impl AsRef<str>>) -> std::process::Output {
    let mut command = Command::new(binary_path());
    for arg in args {
        command.arg(arg.as_ref());
    }
    command.output().expect("rginx command should run")
}

fn parse_counter(output: &str, key: &str) -> u64 {
    output
        .lines()
        .find_map(|line| {
            line.split_whitespace().find_map(|field| field.strip_prefix(&format!("{key}=")))
        })
        .unwrap_or_else(|| panic!("missing counter `{key}` in output: {output}"))
        .parse::<u64>()
        .unwrap_or_else(|error| panic!("invalid counter `{key}`: {error}"))
}

fn fetch_text_response(
    listen_addr: std::net::SocketAddr,
    path: &str,
) -> Result<(u16, String), String> {
    let mut stream = std::net::TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n")
        .map_err(|error| format!("failed to write request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush request: {error}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("failed to read response: {error}"))?;
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

fn send_raw_request(listen_addr: std::net::SocketAddr, request: &str) -> Result<String, String> {
    let mut stream = std::net::TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("failed to write request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush request: {error}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("failed to read response: {error}"))?;
    Ok(response)
}

fn return_config(listen_addr: std::net::SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn explicit_listeners_config(listeners: &[(&str, std::net::SocketAddr)], body: &str) -> String {
    let listeners = listeners
        .iter()
        .map(|(name, addr)| {
            format!(
                "        ListenerConfig(\n            name: {:?},\n            listen: {:?},\n        )",
                name,
                addr.to_string()
            )
        })
        .collect::<Vec<_>>()
        .join(",\n");

    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    listeners: [\n{listeners}\n    ],\n    server: ServerConfig(\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({body:?}),\n            ),\n        ),\n    ],\n)\n",
        listeners = listeners,
        ready_route = READY_ROUTE_CONFIG,
        body = body,
    )
}

fn proxy_config(listen_addr: std::net::SocketAddr, upstream_addr: std::net::SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn spawn_response_server(body: &'static str) -> std::net::SocketAddr {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test upstream listener should bind");
    let listen_addr = listener.local_addr().expect("listener addr should be available");

    std::thread::spawn(move || {
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };

            std::thread::spawn(move || {
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);

                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            });
        }
    });

    listen_addr
}

fn binary_path() -> std::path::PathBuf {
    std::env::var_os("CARGO_BIN_EXE_rginx")
        .map(std::path::PathBuf::from)
        .expect("cargo should expose the rginx test binary path")
}

fn render_output(output: &std::process::Output) -> String {
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

fn generate_cert(hostname: &str) -> TestCertifiedKey {
    let params = CertificateParams::new(vec![hostname.to_string()])
        .expect("self-signed certificate should generate");
    let signing_key = KeyPair::generate().expect("keypair should generate");
    let cert = params.self_signed(&signing_key).expect("self-signed certificate should generate");
    TestCertifiedKey { cert, signing_key, params }
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

fn generate_crl(issuer: &TestCertifiedKey, revoked_serial: u64) -> CertificateRevocationList {
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
