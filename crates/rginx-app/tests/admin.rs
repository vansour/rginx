#![cfg(unix)]

use std::io::{BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

mod support;

use rginx_runtime::admin::{
    AdminRequest, AdminResponse, RevisionSnapshot, admin_socket_path_for_config,
};
use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn local_admin_socket_serves_revision_snapshot() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-uds", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetRevision)
        .expect("admin socket should return revision");
    assert_eq!(response, AdminResponse::Revision(RevisionSnapshot { revision: 0 }));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn status_command_reads_local_admin_socket() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-status", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(output.status.success(), "status command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("revision=0"));
    assert!(stdout.contains(&format!("listen={listen_addr}")));
    assert!(stdout.contains("active_connections=0"));
    assert!(stdout.contains("reload_attempts=0"));
    assert!(stdout.contains("last_reload=-"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn counters_command_reports_local_connection_and_response_counters() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-counters", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    fetch_text_response(listen_addr, "/").expect("root request should succeed");
    fetch_text_response(listen_addr, "/missing").expect("missing request should respond");

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "counters"]);
    assert!(output.status.success(), "counters command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let requests = parse_counter(&stdout, "downstream_requests_total");
    let responses_2xx = parse_counter(&stdout, "downstream_responses_2xx_total");
    let responses_4xx = parse_counter(&stdout, "downstream_responses_4xx_total");
    assert!(requests >= 3, "expected at least three requests, got {requests}: {stdout}");
    assert!(
        responses_2xx >= 2,
        "expected at least two 2xx responses, got {responses_2xx}: {stdout}"
    );
    assert!(
        responses_4xx >= 1,
        "expected at least one 4xx response, got {responses_4xx}: {stdout}"
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn peers_command_reports_upstream_health_snapshot() {
    let listen_addr = reserve_loopback_addr();
    let upstream_addr = reserve_loopback_addr();
    let mut server =
        ServerHarness::spawn("rginx-admin-peers", |_| proxy_config(listen_addr, upstream_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "peers"]);
    assert!(output.status.success(), "peers command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("upstream=backend"));
    assert!(stdout.contains(&format!("peer=http://{upstream_addr}")));
    assert!(stdout.contains("available=true"));
    assert!(stdout.contains("backup=false"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

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
        .find_map(|line| line.strip_prefix(&format!("{key}=")))
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

fn return_config(listen_addr: std::net::SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        ready_route = READY_ROUTE_CONFIG,
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
