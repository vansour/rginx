use std::env;
use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn check_succeeds_without_binding_listener() {
    let reserved = TcpListener::bind(("127.0.0.1", 0)).expect("reserved listener should bind");
    let listen_addr = reserved.local_addr().expect("listener addr should be available");

    let temp_dir = temp_dir("rginx-check-test");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("valid.ron");
    write_static_config(&config_path, listen_addr, "checked\n");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);

    assert!(
        output.status.success(),
        "check should succeed without binding the listener: {}",
        render_output(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("configuration is valid"));
    assert!(stdout.contains(&listen_addr.to_string()));

    drop(reserved);
    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn check_returns_error_for_invalid_config() {
    let temp_dir = temp_dir("rginx-check-test");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("invalid.ron");
    fs::write(
        &config_path,
        "Config(runtime: RuntimeConfig(shutdown_timeout_secs: 0), server: ServerConfig(listen: \"127.0.0.1:8080\"), upstreams: [], locations: [])",
    )
    .expect("invalid config should be written");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);

    assert!(!output.status.success(), "check should fail for invalid config");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("shutdown_timeout_secs"));

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn check_returns_error_for_invalid_server_tls_material() {
    let temp_dir = temp_dir("rginx-check-test");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("invalid-tls.ron");
    let cert_path = temp_dir.join("server.crt");
    let key_path = temp_dir.join("server.key");
    fs::write(&cert_path, "not a certificate").expect("invalid cert should be written");
    fs::write(&key_path, "not a private key").expect("invalid key should be written");
    let listen_addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();

    fs::write(
        &config_path,
        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Static(\n                status: Some(200),\n                content_type: Some(\"text/plain; charset=utf-8\"),\n                body: {:?},\n            ),\n        ),\n    ],\n)\n",
            listen_addr.to_string(),
            cert_path.display().to_string(),
            key_path.display().to_string(),
            "checked\n"
        ),
    )
    .expect("invalid TLS config should be written");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);

    assert!(!output.status.success(), "check should fail for invalid server TLS");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("failed to initialize runtime dependencies"));

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn check_reports_total_routes_and_vhosts_for_vhost_config() {
    let temp_dir = temp_dir("rginx-check-test");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("vhosts.ron");
    let listen_addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();

    fs::write(
        &config_path,
        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"default.example.com\"],\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Static(\n                status: Some(200),\n                content_type: Some(\"text/plain; charset=utf-8\"),\n                body: \"default root\\n\",\n            ),\n        ),\n    ],\n    servers: [\n        VirtualHostConfig(\n            server_names: [\"api.example.com\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/users\"),\n                    handler: Static(\n                        status: Some(200),\n                        content_type: Some(\"text/plain; charset=utf-8\"),\n                        body: \"api users\\n\",\n                    ),\n                ),\n                LocationConfig(\n                    matcher: Exact(\"/status\"),\n                    handler: Status,\n                ),\n            ],\n        ),\n        VirtualHostConfig(\n            server_names: [\"*.internal.example.com\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/\"),\n                    handler: Static(\n                        status: Some(200),\n                        content_type: Some(\"text/plain; charset=utf-8\"),\n                        body: \"internal root\\n\",\n                    ),\n                ),\n            ],\n        ),\n    ],\n)\n",
            listen_addr.to_string()
        ),
    )
    .expect("vhost config should be written");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);

    assert!(output.status.success(), "check should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("vhosts=3"), "stdout should report total vhost count: {stdout}");
    assert!(stdout.contains("routes=4"), "stdout should report total route count: {stdout}");

    let _ = fs::remove_dir_all(temp_dir);
}

fn run_rginx(args: impl IntoIterator<Item = impl AsRef<str>>) -> Output {
    let mut command = Command::new(binary_path());
    for arg in args {
        command.arg(arg.as_ref());
    }

    command.output().expect("rginx should run")
}

fn write_static_config(path: &Path, listen_addr: SocketAddr, body: &str) {
    fs::write(path, static_config(listen_addr, body)).expect("config file should be written");
}

fn static_config(listen_addr: SocketAddr, body: &str) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Static(\n                status: Some(200),\n                content_type: Some(\"text/plain; charset=utf-8\"),\n                body: {:?},\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        body
    )
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

fn binary_path() -> PathBuf {
    env::var_os("CARGO_BIN_EXE_rginx")
        .map(PathBuf::from)
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
