use std::env;
use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rcgen::{
    BasicConstraints, CertificateParams, CertifiedKey, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair,
};

#[test]
fn check_succeeds_without_binding_listener() {
    let reserved = TcpListener::bind(("127.0.0.1", 0)).expect("reserved listener should bind");
    let listen_addr = reserved.local_addr().expect("listener addr should be available");

    let temp_dir = temp_dir("rginx-check-test");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("valid.ron");
    write_return_config(&config_path, listen_addr, "checked\n");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);

    assert!(
        output.status.success(),
        "check should succeed without binding the listener: {}",
        render_output(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("configuration is valid"));
    assert!(stdout.contains("listener_model=legacy"));
    assert!(stdout.contains("listeners=1"));
    assert!(stdout.contains(&listen_addr.to_string()));
    assert!(stdout.contains("worker_threads=auto"));
    assert!(stdout.contains("accept_workers=1"));
    assert!(stdout.contains(
        "reload_requires_restart_for=listen,listeners[].listen,runtime.worker_threads,runtime.accept_workers"
    ));
    assert!(stdout.contains(
        "reload_tls_updates=server.tls,listeners[].tls,servers[].tls,upstreams[].tls,upstreams[].server_name,upstreams[].server_name_override"
    ));
    assert!(stdout.contains("tls_expiring_certificates=-"));

    drop(reserved);
    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn nginx_style_t_flag_succeeds_without_binding_listener() {
    let reserved = TcpListener::bind(("127.0.0.1", 0)).expect("reserved listener should bind");
    let listen_addr = reserved.local_addr().expect("listener addr should be available");

    let temp_dir = temp_dir("rginx-check-test");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("valid.ron");
    write_return_config(&config_path, listen_addr, "checked\n");

    let output = run_rginx(["-t", "--config", config_path.to_str().unwrap()]);

    assert!(output.status.success(), "-t should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("configuration is valid"));
    assert!(stdout.contains("listener_model=legacy"));
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
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({:?}),\n            ),\n        ),\n    ],\n)\n",
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
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"default.example.com\"],\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"default root\\n\"),\n            ),\n        ),\n    ],\n    servers: [\n        VirtualHostConfig(\n            server_names: [\"api.example.com\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/users\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"api users\\n\"),\n                    ),\n                ),\n                LocationConfig(\n                    matcher: Exact(\"/healthz\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"ok\\n\"),\n                    ),\n                ),\n            ],\n        ),\n        VirtualHostConfig(\n            server_names: [\"*.internal.example.com\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"internal root\\n\"),\n                    ),\n                ),\n            ],\n        ),\n    ],\n)\n",
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

#[test]
fn check_reports_runtime_worker_settings() {
    let temp_dir = temp_dir("rginx-check-test");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("runtime-workers.ron");
    let listen_addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();

    fs::write(
        &config_path,
        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n        worker_threads: Some(4),\n        accept_workers: Some(2),\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"checked\\n\"),\n            ),\n        ),\n    ],\n)\n",
            listen_addr.to_string()
        ),
    )
    .expect("runtime worker config should be written");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);

    assert!(output.status.success(), "check should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("worker_threads=4"));
    assert!(stdout.contains("accept_workers=2"));
    assert!(stdout.contains("listener_model=legacy"));

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn check_reports_explicit_listener_summary_and_reload_boundary() {
    let temp_dir = temp_dir("rginx-check-listeners-test");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("listeners.ron");
    let http_addr: SocketAddr = "127.0.0.1:18080".parse().unwrap();
    let https_addr: SocketAddr = "127.0.0.1:18443".parse().unwrap();

    fs::write(
        &config_path,
        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n        worker_threads: Some(3),\n        accept_workers: Some(2),\n    ),\n    listeners: [\n        ListenerConfig(\n            name: \"http\",\n            listen: {:?},\n        ),\n        ListenerConfig(\n            name: \"https\",\n            listen: {:?},\n        ),\n    ],\n    server: ServerConfig(\n        server_names: [\"example.com\"],\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"checked\\n\"),\n            ),\n        ),\n    ],\n)\n",
            http_addr.to_string(),
            https_addr.to_string()
        ),
    )
    .expect("explicit listener config should be written");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);

    assert!(output.status.success(), "check should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("listener_model=explicit"));
    assert!(stdout.contains("listeners=2"));
    assert!(stdout.contains("listen=127.0.0.1:18080"));
    assert!(stdout.contains("worker_threads=3"));
    assert!(stdout.contains("accept_workers=2"));
    assert!(stdout.contains(
        "reload_requires_restart_for=listen,listeners[].listen,runtime.worker_threads,runtime.accept_workers"
    ));
    assert!(stdout.contains(
        "tls_restart_required_fields=listen,listeners[].listen,runtime.worker_threads,runtime.accept_workers"
    ));

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn check_reports_tls_diagnostics_for_listener_and_vhost_certificates() {
    let temp_dir = temp_dir("rginx-check-tls-diagnostics");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("tls-diagnostics.ron");
    let listen_addr: SocketAddr = "127.0.0.1:18443".parse().unwrap();

    let primary = generate_cert("default.example.com");
    let primary_cert_path = temp_dir.join("default.crt");
    let primary_key_path = temp_dir.join("default.key");
    fs::write(&primary_cert_path, primary.cert.pem()).expect("primary cert should be written");
    fs::write(&primary_key_path, primary.signing_key.serialize_pem())
        .expect("primary key should be written");

    let vhost = generate_cert("api.example.com");
    let vhost_cert_path = temp_dir.join("api.crt");
    let vhost_key_path = temp_dir.join("api.key");
    fs::write(&vhost_cert_path, vhost.cert.pem()).expect("vhost cert should be written");
    fs::write(&vhost_key_path, vhost.signing_key.serialize_pem())
        .expect("vhost key should be written");

    fs::write(
        &config_path,
        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"default.example.com\"],\n        default_certificate: Some(\"api.example.com\"),\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n    servers: [\n        VirtualHostConfig(\n            server_names: [\"api.example.com\"],\n            locations: [\n                LocationConfig(\n                    matcher: Exact(\"/\"),\n                    handler: Return(\n                        status: 200,\n                        location: \"\",\n                        body: Some(\"api\\n\"),\n                    ),\n                ),\n            ],\n            tls: Some(VirtualHostTlsConfig(\n                cert_path: {:?},\n                key_path: {:?},\n            )),\n        ),\n    ],\n)\n",
            listen_addr.to_string(),
            primary_cert_path.display().to_string(),
            primary_key_path.display().to_string(),
            vhost_cert_path.display().to_string(),
            vhost_key_path.display().to_string(),
        ),
    )
    .expect("TLS diagnostics config should be written");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);
    assert!(output.status.success(), "check should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(
        "tls_details=listener_profiles=1 vhost_overrides=1 sni_names=2 certificate_bundles=2"
    ));
    assert!(stdout.contains(
        "reload_tls_updates=server.tls,listeners[].tls,servers[].tls,upstreams[].tls,upstreams[].server_name,upstreams[].server_name_override"
    ));
    assert!(stdout.contains("tls_default_certificates=default=api.example.com"));
    assert!(stdout.contains("tls_expiring_certificates=-"));
    assert!(stdout.contains("tls_certificate scope=listener:default sha256="));
    assert!(stdout.contains("tls_sni_binding listener=default server_name=api.example.com"));
    assert!(
        stdout.contains(
            "tls_default_certificate_binding listener=default server_name=api.example.com"
        )
    );
    assert!(stdout.contains("tls_sni_conflicts=-"));

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn check_returns_error_for_mismatched_server_tls_key() {
    let temp_dir = temp_dir("rginx-check-test");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("mismatched-tls.ron");
    let cert_path = temp_dir.join("server.crt");
    let key_path = temp_dir.join("server.key");
    let listen_addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();

    let cert_pair = generate_cert("localhost");
    let other_key = KeyPair::generate().expect("other key should generate");
    fs::write(&cert_path, cert_pair.cert.pem()).expect("cert should be written");
    fs::write(&key_path, other_key.serialize_pem()).expect("mismatched key should be written");

    fs::write(
        &config_path,
        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"checked\\n\"),\n            ),\n        ),\n    ],\n)\n",
            listen_addr.to_string(),
            cert_path.display().to_string(),
            key_path.display().to_string(),
        ),
    )
    .expect("mismatched TLS config should be written");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);

    assert!(!output.status.success(), "check should fail for mismatched TLS key");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("does not match private key"),
        "stderr should explain mismatch: {stderr}"
    );

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn check_reports_certificate_fingerprint_and_chain_diagnostics() {
    let temp_dir = temp_dir("rginx-check-chain-diagnostics");
    fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
    let config_path = temp_dir.join("chain-diagnostics.ron");
    let cert_path = temp_dir.join("leaf.crt");
    let key_path = temp_dir.join("leaf.key");
    let listen_addr: SocketAddr = "127.0.0.1:18444".parse().unwrap();

    let cert_pair = generate_cert_signed_by_ca("leaf.example.com");
    fs::write(&cert_path, cert_pair.cert.pem()).expect("leaf cert should be written");
    fs::write(&key_path, cert_pair.signing_key.serialize_pem())
        .expect("leaf key should be written");

    fs::write(
        &config_path,
        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"leaf.example.com\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"checked\\n\"),\n            ),\n        ),\n    ],\n)\n",
            listen_addr.to_string(),
            cert_path.display().to_string(),
            key_path.display().to_string(),
        ),
    )
    .expect("chain diagnostics config should be written");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);
    assert!(output.status.success(), "check should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("tls_certificate scope=listener:default sha256="));
    assert!(stdout.contains("chain_length=1"));
    assert!(stdout.contains("chain_incomplete_single_non_self_signed_certificate"));

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn check_supports_relative_includes_and_environment_expansion() {
    let temp_dir = temp_dir("rginx-check-include-env-test");
    fs::create_dir_all(temp_dir.join("fragments")).expect("temp fragments dir should be created");
    let config_path = temp_dir.join("rginx.ron");
    let routes_path = temp_dir.join("fragments/routes.ron");
    let listen_addr: SocketAddr = "127.0.0.1:18082".parse().unwrap();

    fs::write(
        &routes_path,
        "LocationConfig(\n    matcher: Exact(\"/\"),\n    handler: Return(\n        status: 200,\n        location: \"\",\n        body: Some(\"${rginx_check_body:-included body\\n}\"),\n    ),\n),\n",
    )
    .expect("routes fragment should be written");
    fs::write(
        &config_path,
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: \"${rginx_check_listen}\",\n    ),\n    upstreams: [],\n    locations: [\n        // @include \"fragments/routes.ron\"\n    ],\n)\n",
    )
    .expect("root config should be written");

    let output = Command::new(binary_path())
        .env("rginx_check_listen", listen_addr.to_string())
        .env("rginx_check_body", "body from env\n")
        .arg("check")
        .arg("--config")
        .arg(&config_path)
        .output()
        .expect("rginx should run");

    assert!(output.status.success(), "check should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&listen_addr.to_string()));

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn check_succeeds_for_repository_default_config() {
    let workspace_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve");
    let config_path = workspace_root.join("configs/rginx.ron");

    let output = run_rginx(["check", "--config", config_path.to_str().unwrap()]);

    assert!(
        output.status.success(),
        "repository default config should validate: {}",
        render_output(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("listen=0.0.0.0:80"));
    assert!(stdout.contains("routes=5"));
    assert!(stdout.contains("vhosts=2"));
    assert!(stdout.contains("upstreams=1"));
}

fn run_rginx(args: impl IntoIterator<Item = impl AsRef<str>>) -> Output {
    let mut command = Command::new(binary_path());
    for arg in args {
        command.arg(arg.as_ref());
    }

    command.output().expect("rginx should run")
}

fn write_return_config(path: &Path, listen_addr: SocketAddr, body: &str) {
    fs::write(path, return_config(listen_addr, body)).expect("config file should be written");
}

fn return_config(listen_addr: SocketAddr, body: &str) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({:?}),\n            ),\n        ),\n    ],\n)\n",
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

type TestCertifiedKey = CertifiedKey<KeyPair>;

fn generate_cert(hostname: &str) -> TestCertifiedKey {
    rcgen::generate_simple_self_signed(vec![hostname.to_string()])
        .expect("self-signed certificate should generate")
}

fn generate_cert_signed_by_ca(hostname: &str) -> TestCertifiedKey {
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.distinguished_name.push(DnType::CommonName, "check test ca");
    let ca_key = KeyPair::generate().expect("CA key should generate");
    let ca_cert = ca_params.self_signed(&ca_key).expect("CA cert should generate");
    let _ca = CertifiedKey { cert: ca_cert, signing_key: ca_key };
    let ca_issuer = rcgen::Issuer::from_params(&ca_params, &_ca.signing_key);

    let mut leaf_params =
        CertificateParams::new(vec![hostname.to_string()]).expect("leaf params should build");
    leaf_params.distinguished_name.push(DnType::CommonName, hostname);
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf_key = KeyPair::generate().expect("leaf key should generate");
    let cert = leaf_params.signed_by(&leaf_key, &ca_issuer).expect("leaf cert should be signed");
    CertifiedKey { cert, signing_key: leaf_key }
}
