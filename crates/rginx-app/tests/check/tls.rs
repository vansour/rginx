use super::*;

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
        "reload_tls_updates=server.tls,server.http3.advertise_alt_svc,server.http3.alt_svc_max_age_secs,listeners[].tls,listeners[].http3.advertise_alt_svc,listeners[].http3.alt_svc_max_age_secs,servers[].tls,upstreams[].tls,upstreams[].server_name,upstreams[].server_name_override"
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
