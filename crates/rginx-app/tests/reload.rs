#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use rcgen::CertifiedKey;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[test]
fn sighup_reload_applies_updated_routes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "before reload\n");

    server.wait_for_body(listen_addr, "before reload\n", Duration::from_secs(5));

    server.write_return_config(listen_addr, "after reload\n");
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(listen_addr, "after reload\n", Duration::from_secs(5));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn nginx_style_reload_command_applies_updated_routes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "before reload\n");

    server.wait_for_body(listen_addr, "before reload\n", Duration::from_secs(5));

    server.write_return_config(listen_addr, "after reload\n");
    let output = server.send_cli_signal("reload");

    assert!(output.status.success(), "rginx -s reload should succeed: {}", render_output(&output));

    server.wait_for_body(listen_addr, "after reload\n", Duration::from_secs(5));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn nginx_style_quit_command_stops_the_server() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "before quit\n");

    server.wait_for_body(listen_addr, "before quit\n", Duration::from_secs(5));

    let output = server.send_cli_signal("quit");
    assert!(output.status.success(), "rginx -s quit should succeed: {}", render_output(&output));

    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success(), "rginx should exit cleanly after quit: {status}");
}

#[test]
fn sighup_reload_adds_explicit_listener_without_restart() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let http_addr = reserve_loopback_addr();
    let admin_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn_with_config(
        "rginx-reload-add-listener",
        explicit_listeners_config(&[("http", http_addr)], "before add\n"),
    );

    server.wait_for_body(http_addr, "before add\n", Duration::from_secs(5));
    assert_unreachable(admin_addr, Duration::from_millis(500));

    server.write_config(explicit_listeners_config(
        &[("http", http_addr), ("admin", admin_addr)],
        "after add\n",
    ));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(http_addr, "after add\n", Duration::from_secs(5));
    server.wait_for_body(admin_addr, "after add\n", Duration::from_secs(5));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_reload_removes_explicit_listener_without_restart() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let http_addr = reserve_loopback_addr();
    let admin_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn_with_config(
        "rginx-reload-remove-listener",
        explicit_listeners_config(&[("http", http_addr), ("admin", admin_addr)], "before remove\n"),
    );

    server.wait_for_body(http_addr, "before remove\n", Duration::from_secs(5));
    server.wait_for_body(admin_addr, "before remove\n", Duration::from_secs(5));

    server.write_config(explicit_listeners_config(&[("http", http_addr)], "after remove\n"));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(http_addr, "after remove\n", Duration::from_secs(5));
    assert_unreachable(admin_addr, Duration::from_secs(2));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn removed_listener_drains_in_flight_request_before_going_unreachable() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let http_addr = reserve_loopback_addr();
    let drain_addr = reserve_loopback_addr();
    let (ready_tx, ready_rx) = mpsc::channel();
    let upstream_addr =
        spawn_delayed_response_server(Duration::from_millis(300), "draining\n", Some(ready_tx));
    let mut server = TestServer::spawn_with_config(
        "rginx-reload-drain-listener",
        explicit_listeners_proxy_config(
            &[("http", http_addr), ("drain", drain_addr)],
            upstream_addr,
        ),
    );

    server.wait_for_body(http_addr, "draining\n", Duration::from_secs(5));
    server.wait_for_body(drain_addr, "draining\n", Duration::from_secs(5));
    while ready_rx.try_recv().is_ok() {}

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        tx.send(fetch_text_response_with_timeout(drain_addr, "/", Duration::from_secs(3)))
            .expect("result channel should remain available");
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).expect("in-flight request should reach upstream");

    server.write_config(explicit_listeners_proxy_config(&[("http", http_addr)], upstream_addr));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(http_addr, "draining\n", Duration::from_secs(5));
    let result = rx.recv_timeout(Duration::from_secs(5)).expect("in-flight request should finish");
    let (status, body) = result.expect("in-flight request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "draining\n");

    assert_unreachable(drain_addr, Duration::from_secs(2));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_rejects_listen_address_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let initial_addr = reserve_loopback_addr();
    let rejected_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(initial_addr, "stable config\n");

    server.wait_for_body(initial_addr, "stable config\n", Duration::from_secs(5));

    server.write_return_config(rejected_addr, "should not apply\n");
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(initial_addr, "stable config\n", Duration::from_secs(5));
    assert_unreachable(rejected_addr, Duration::from_millis(500));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_rejects_accept_worker_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "stable workers\n");

    server.wait_for_body(listen_addr, "stable workers\n", Duration::from_secs(5));

    server.write_config(return_config_with_runtime(
        listen_addr,
        "should not apply\n",
        "        accept_workers: Some(2),\n",
    ));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(listen_addr, "stable workers\n", Duration::from_secs(5));
    server.kill_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_rejects_runtime_worker_thread_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "stable runtime\n");

    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));

    server.write_config(return_config_with_runtime(
        listen_addr,
        "should not apply\n",
        "        worker_threads: Some(2),\n",
    ));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));
    server.kill_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_status_reports_restart_required_fields_for_startup_boundary_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "stable runtime\n");

    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));

    server.write_config(return_config_with_runtime(
        listen_addr,
        "should not apply\n",
        "        worker_threads: Some(2),\n        accept_workers: Some(2),\n",
    ));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));
    let status_output = server.run_cli_command(["status"]);
    assert!(
        status_output.status.success(),
        "rginx status should succeed after rejected reload: {}",
        render_output(&status_output)
    );
    let stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(stdout.contains("reload_failures=1"), "stdout should report reload failure: {stdout}");
    assert!(
        stdout.contains("last_reload_active_revision=0"),
        "stdout should report the preserved active revision: {stdout}"
    );
    assert!(
        stdout.contains("last_reload_rollback_revision=0"),
        "stdout should report rollback preservation: {stdout}"
    );
    assert!(
        stdout.contains("reload requires restart because these startup-boundary fields changed"),
        "stdout should explain restart boundary: {stdout}"
    );
    assert!(
        stdout.contains("runtime.worker_threads"),
        "stdout should mention worker_threads: {stdout}"
    );
    assert!(
        stdout.contains("runtime.accept_workers"),
        "stdout should mention accept_workers: {stdout}"
    );

    server.kill_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_reload_picks_up_updated_included_fragments() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn_with_setup("rginx-reload-include-test", |temp_dir| {
        fs::write(temp_dir.join("routes.ron"), return_route_fragment("before include reload\n"))
            .expect("initial routes fragment should be written");
        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        // @include \"routes.ron\"\n    ],\n)\n",
            listen_addr.to_string(),
            ready_route = READY_ROUTE_CONFIG,
        )
    });
    let routes_path = server.temp_dir().join("routes.ron");

    server.wait_for_body(listen_addr, "before include reload\n", Duration::from_secs(5));

    fs::write(&routes_path, return_route_fragment("after include reload\n"))
        .expect("updated routes fragment should be written");
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(listen_addr, "after include reload\n", Duration::from_secs(5));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn nginx_style_restart_command_applies_listen_address_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let initial_addr = reserve_loopback_addr();
    let restarted_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(initial_addr, "before restart\n");

    server.wait_for_body(initial_addr, "before restart\n", Duration::from_secs(5));
    let old_pid = read_pid_file(&server.pid_path());

    server.write_return_config(restarted_addr, "after restart\n");
    let output = server.send_cli_signal("restart");
    assert!(output.status.success(), "rginx -s restart should succeed: {}", render_output(&output));

    let new_pid = wait_for_pid_change(&server.pid_path(), old_pid, Duration::from_secs(10));
    let status = server.wait_for_exit(Duration::from_secs(10));
    assert!(status.success(), "old process should exit cleanly after restart: {status}");

    wait_for_body(restarted_addr, "after restart\n", Duration::from_secs(10));
    assert_unreachable(initial_addr, Duration::from_millis(500));

    let quit = server.send_cli_signal("quit");
    assert!(
        quit.status.success(),
        "rginx -s quit should stop replacement process: {}",
        render_output(&quit)
    );
    wait_for_process_exit(new_pid, Duration::from_secs(10));
}

#[test]
fn nginx_style_restart_command_applies_runtime_worker_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "runtime restart\n");

    server.wait_for_body(listen_addr, "runtime restart\n", Duration::from_secs(5));
    let old_pid = read_pid_file(&server.pid_path());

    server.write_config(return_config_with_runtime(
        listen_addr,
        "runtime restart\n",
        "        worker_threads: Some(2),\n        accept_workers: Some(2),\n",
    ));
    let output = server.send_cli_signal("restart");
    assert!(output.status.success(), "rginx -s restart should succeed: {}", render_output(&output));

    let new_pid = wait_for_pid_change(&server.pid_path(), old_pid, Duration::from_secs(10));
    let status = server.wait_for_exit(Duration::from_secs(10));
    assert!(status.success(), "old process should exit cleanly after restart: {status}");

    wait_for_body(listen_addr, "runtime restart\n", Duration::from_secs(10));
    let status_output = server.run_cli_command(["status"]);
    assert!(
        status_output.status.success(),
        "rginx status should succeed after restart: {}",
        render_output(&status_output)
    );
    let stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(stdout.contains("worker_threads=2"));
    assert!(stdout.contains("accept_workers=2"));

    let quit = server.send_cli_signal("quit");
    assert!(
        quit.status.success(),
        "rginx -s quit should stop replacement process: {}",
        render_output(&quit)
    );
    wait_for_process_exit(new_pid, Duration::from_secs(10));
}

#[test]
fn nginx_style_restart_command_keeps_old_process_running_when_replacement_fails() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "stable runtime\n");

    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));
    let old_pid = read_pid_file(&server.pid_path());

    server.write_config(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 0,\n    ),\n    server: ServerConfig(\n        listen: \"127.0.0.1:0\",\n    ),\n    upstreams: [],\n    locations: [],\n)\n".to_string(),
    );
    let output = server.send_cli_signal("restart");
    assert!(
        output.status.success(),
        "restart signal delivery should still succeed: {}",
        render_output(&output)
    );

    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(read_pid_file(&server.pid_path()), old_pid);
    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_status_reports_tls_certificate_changes_after_rotation() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let initial_cert = generate_cert("localhost");
    let rotated_cert = generate_cert("localhost");
    let mut server = ServerHarness::spawn("rginx-reload-tls-rotation", |temp_dir| {
        let cert_path = temp_dir.join("server.crt");
        let key_path = temp_dir.join("server.key");
        fs::write(&cert_path, initial_cert.cert.pem()).expect("initial cert should be written");
        fs::write(&key_path, initial_cert.key_pair.serialize_pem())
            .expect("initial key should be written");
        tls_return_config(listen_addr, &cert_path, &key_path)
    });

    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let rotated_cert_path = server.temp_dir().join("server-rotated.crt");
    let rotated_key_path = server.temp_dir().join("server-rotated.key");
    fs::write(&rotated_cert_path, rotated_cert.cert.pem()).expect("rotated cert should be written");
    fs::write(&rotated_key_path, rotated_cert.key_pair.serialize_pem())
        .expect("rotated key should be written");
    fs::write(
        server.config_path(),
        tls_return_config(listen_addr, &rotated_cert_path, &rotated_key_path),
    )
    .expect("rotated TLS config should be written");
    server.send_signal(libc::SIGHUP);

    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));
    let status_output = run_cli_command(server.config_path(), ["status"]);
    assert!(
        status_output.status.success(),
        "rginx status should succeed after certificate rotation reload: {}",
        render_output(&status_output)
    );
    let stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(stdout.contains("reload_successes=1"), "stdout should report reload success: {stdout}");
    assert!(
        stdout.contains("last_reload_tls_certificate_changes=")
            && stdout.contains("listener:default:")
            && stdout.contains("->"),
        "stdout should report TLS certificate changes: {stdout}"
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

struct TestServer {
    inner: ServerHarness,
}

impl TestServer {
    fn spawn(listen_addr: SocketAddr, body: &str) -> Self {
        Self::spawn_with_config("rginx-test", return_config(listen_addr, body))
    }

    fn spawn_with_config(prefix: &str, config: String) -> Self {
        Self::spawn_with_setup(prefix, |_| config)
    }

    fn spawn_with_setup(prefix: &str, setup: impl FnOnce(&Path) -> String) -> Self {
        Self { inner: ServerHarness::spawn(prefix, setup) }
    }

    fn write_return_config(&self, listen_addr: SocketAddr, body: &str) {
        write_return_config(self.inner.config_path(), listen_addr, body);
    }

    fn write_config(&self, config: String) {
        fs::write(self.inner.config_path(), config).expect("config file should be written");
    }

    fn wait_for_body(&mut self, listen_addr: SocketAddr, expected: &str, timeout: Duration) {
        self.inner.wait_for_http_text_response(
            listen_addr,
            &listen_addr.to_string(),
            "/",
            200,
            expected,
            timeout,
        );
    }

    fn send_signal(&self, signal: i32) {
        self.inner.send_signal(signal);
    }

    fn shutdown_and_wait(&mut self, timeout: Duration) {
        self.inner.terminate_and_wait(timeout);
    }

    fn kill_and_wait(&mut self, timeout: Duration) {
        self.inner.kill_and_wait(timeout);
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> std::process::ExitStatus {
        self.inner.wait_for_exit(timeout)
    }

    fn temp_dir(&self) -> &Path {
        self.inner.temp_dir()
    }

    fn pid_path(&self) -> PathBuf {
        self.inner.config_path().with_extension("pid")
    }

    fn send_cli_signal(&self, signal: &str) -> Output {
        Command::new(binary_path())
            .arg("--config")
            .arg(self.inner.config_path())
            .arg("-s")
            .arg(signal)
            .output()
            .expect("rginx signal command should run")
    }

    fn run_cli_command<'a>(&self, args: impl IntoIterator<Item = &'a str>) -> Output {
        let mut command = Command::new(binary_path());
        command.arg("--config").arg(self.inner.config_path());
        for arg in args {
            command.arg(arg);
        }
        command.output().expect("rginx command should run")
    }
}

fn write_return_config(path: &Path, listen_addr: SocketAddr, body: &str) {
    fs::write(path, return_config(listen_addr, body)).expect("config file should be written");
}

fn return_config(listen_addr: SocketAddr, body: &str) -> String {
    return_config_with_runtime(listen_addr, body, "")
}

fn return_config_with_runtime(listen_addr: SocketAddr, body: &str, runtime_extra: &str) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n{}    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({:?}),\n            ),\n        ),\n    ],\n)\n",
        runtime_extra,
        listen_addr.to_string(),
        body,
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn return_route_fragment(body: &str) -> String {
    format!(
        "LocationConfig(\n    matcher: Exact(\"/\"),\n    handler: Return(\n        status: 200,\n        location: \"\",\n        body: Some({:?}),\n    ),\n),\n",
        body
    )
}

fn explicit_listeners_config(listeners: &[(&str, SocketAddr)], body: &str) -> String {
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

fn explicit_listeners_proxy_config(
    listeners: &[(&str, SocketAddr)],
    upstream_addr: SocketAddr,
) -> String {
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
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    listeners: [\n{listeners}\n    ],\n    server: ServerConfig(\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {upstream:?},\n                ),\n            ],\n            request_timeout_secs: Some(3),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listeners = listeners,
        upstream = format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn fetch_text_response(listen_addr: SocketAddr, path: &str) -> Result<(u16, String), String> {
    fetch_text_response_with_timeout(listen_addr, path, Duration::from_millis(500))
}

fn fetch_text_response_with_timeout(
    listen_addr: SocketAddr,
    path: &str,
    read_timeout: Duration,
) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(read_timeout))
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

fn spawn_delayed_response_server(
    delay: Duration,
    body: &'static str,
    notify_ready: Option<mpsc::Sender<()>>,
) -> SocketAddr {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("delayed upstream listener should bind");
    let listen_addr = listener.local_addr().expect("listener addr should be available");

    std::thread::spawn(move || {
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let notify_ready = notify_ready.clone();

            std::thread::spawn(move || {
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                if let Some(notify_ready) = &notify_ready {
                    let _ = notify_ready.send(());
                }
                std::thread::sleep(delay);

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

fn assert_unreachable(listen_addr: SocketAddr, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;

    while std::time::Instant::now() < deadline {
        match fetch_text_response(listen_addr, "/") {
            Ok((status, body)) => {
                panic!(
                    "expected {} to stay unreachable, got status={} body={:?}",
                    listen_addr, status, body
                );
            }
            Err(_) => {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn wait_for_body(listen_addr: SocketAddr, expected: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut last_error = String::new();

    while Instant::now() < deadline {
        match fetch_text_response(listen_addr, "/") {
            Ok((200, body)) if body == expected => return,
            Ok((status, body)) => {
                last_error = format!("unexpected response: status={status} body={body:?}");
            }
            Err(error) => last_error = error,
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    panic!(
        "timed out waiting for expected response on {}; expected body {:?}; last error: {}",
        listen_addr, expected, last_error
    );
}

fn read_pid_file(path: &Path) -> i32 {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read pid file {}: {error}", path.display()))
        .trim()
        .parse::<i32>()
        .unwrap_or_else(|error| panic!("invalid pid file {}: {error}", path.display()))
}

fn wait_for_pid_change(path: &Path, old_pid: i32, timeout: Duration) -> i32 {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if path.exists() {
            let pid = read_pid_file(path);
            if pid != old_pid {
                return pid;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    panic!("timed out waiting for pid file {} to move away from pid {}", path.display(), old_pid);
}

fn wait_for_process_exit(pid: i32, timeout: Duration) {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        let result = unsafe { libc::kill(pid, 0) };
        if result != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return;
            }
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    panic!("timed out waiting for pid {pid} to exit");
}

fn binary_path() -> std::path::PathBuf {
    std::env::var_os("CARGO_BIN_EXE_rginx")
        .map(std::path::PathBuf::from)
        .expect("cargo should expose the rginx test binary path")
}

fn render_output(output: &Output) -> String {
    format!(
        "status={}; stdout={:?}; stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn generate_cert(hostname: &str) -> CertifiedKey {
    let cert = rcgen::generate_simple_self_signed(vec![hostname.to_string()])
        .expect("self-signed certificate should generate");
    CertifiedKey { cert: cert.cert, key_pair: cert.key_pair }
}

fn tls_return_config(listen_addr: SocketAddr, cert_path: &Path, key_path: &Path) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"tls reload\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn run_cli_command<'a>(config_path: &Path, args: impl IntoIterator<Item = &'a str>) -> Output {
    let mut command = Command::new(binary_path());
    command.arg("--config").arg(config_path);
    for arg in args {
        command.arg(arg);
    }
    command.output().expect("rginx command should run")
}

fn test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
