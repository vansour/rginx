#![cfg(unix)]

use std::io::{BufReader, Read, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use rcgen::{
    BasicConstraints, CertificateParams, CertificateRevocationList,
    CertificateRevocationListParams, CertifiedKey, DnType, IsCa, KeyIdMethod, KeyPair,
    KeyUsagePurpose, RevocationReason, RevokedCertParams, SerialNumber, date_time_ymd,
};

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
fn snapshot_command_returns_aggregate_json_snapshot() {
    let listen_addr = reserve_loopback_addr();
    let upstream_addr = spawn_response_server("snapshot upstream ok\n");
    let mut server =
        ServerHarness::spawn("rginx-admin-snapshot", |_| proxy_config(listen_addr, upstream_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let (status, body) =
        fetch_text_response(listen_addr, "/api/demo").expect("proxy request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "snapshot upstream ok\n");

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::GetSnapshot { include: None, window_secs: None },
    )
    .expect("admin socket should return aggregate snapshot");
    let AdminResponse::Snapshot(snapshot) = response else {
        panic!("admin socket should return aggregate snapshot");
    };
    assert_eq!(snapshot.schema_version, 10);
    assert!(snapshot.captured_at_unix_ms > 0);
    assert!(snapshot.pid > 0);
    assert_eq!(snapshot.binary_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(snapshot.included_modules, rginx_http::SnapshotModule::all());
    assert_eq!(snapshot.status.as_ref().map(|status| status.listen_addr), Some(listen_addr));
    assert_eq!(snapshot.status.as_ref().map(|status| status.tls.listeners.len()), Some(1));
    assert!(snapshot.counters.as_ref().map(|c| c.downstream_requests).unwrap_or(0) >= 1);
    assert_eq!(snapshot.traffic.as_ref().map(|t| t.listeners.len()), Some(1));
    assert_eq!(snapshot.peer_health.as_ref().map(Vec::len), Some(1));
    assert_eq!(snapshot.upstreams.as_ref().map(Vec::len), Some(1));

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "snapshot"]);
    assert!(output.status.success(), "snapshot command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let snapshot: serde_json::Value =
        serde_json::from_str(&stdout).expect("snapshot command should print valid JSON");
    assert_eq!(snapshot["schema_version"], serde_json::Value::from(10));
    assert!(snapshot["captured_at_unix_ms"].as_u64().unwrap_or(0) > 0);
    assert!(snapshot["pid"].as_u64().unwrap_or(0) > 0);
    assert_eq!(snapshot["binary_version"], serde_json::Value::from(env!("CARGO_PKG_VERSION")));
    assert_eq!(snapshot["status"]["listen_addr"], serde_json::Value::from(listen_addr.to_string()));
    assert_eq!(snapshot["status"]["tls"]["listeners"].as_array().map(Vec::len), Some(1));
    assert!(snapshot["counters"]["downstream_requests"].as_u64().unwrap_or(0) >= 1);
    assert_eq!(snapshot["traffic"]["listeners"].as_array().map(Vec::len), Some(1));
    assert_eq!(snapshot["peer_health"].as_array().map(Vec::len), Some(1));
    assert_eq!(snapshot["upstreams"].as_array().map(Vec::len), Some(1));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn snapshot_includes_certificate_fingerprint_and_chain_details_for_tls_servers() {
    let listen_addr = reserve_loopback_addr();
    let cert = generate_cert("localhost");
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-admin-tls-snapshot",
        &cert.cert.pem(),
        &cert.key_pair.serialize_pem(),
        |_, cert_path, key_path| {
            format!(
                "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
                listen_addr.to_string(),
                cert_path.display().to_string(),
                key_path.display().to_string(),
                ready_route = READY_ROUTE_CONFIG,
            )
        },
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::GetSnapshot { include: None, window_secs: None },
    )
    .expect("admin socket should return aggregate snapshot");
    let AdminResponse::Snapshot(snapshot) = response else {
        panic!("admin socket should return aggregate snapshot");
    };
    let certificates =
        snapshot.status.as_ref().map(|status| status.tls.certificates.as_slice()).unwrap_or(&[]);
    let vhost_bindings =
        snapshot.status.as_ref().map(|status| status.tls.vhost_bindings.as_slice()).unwrap_or(&[]);
    let sni_bindings =
        snapshot.status.as_ref().map(|status| status.tls.sni_bindings.as_slice()).unwrap_or(&[]);
    assert_eq!(certificates.len(), 1);
    assert_eq!(vhost_bindings.len(), 1);
    assert_eq!(sni_bindings.len(), 1);
    let certificate = &certificates[0];
    assert_eq!(certificate.scope, "listener:default");
    assert!(certificate.subject.is_some());
    assert_eq!(certificate.san_dns_names, vec!["localhost".to_string()]);
    assert_eq!(certificate.subject, certificate.issuer);
    assert!(certificate.fingerprint_sha256.as_ref().is_some_and(|value| value.len() == 64));
    assert_eq!(certificate.chain_length, 1);
    assert_eq!(certificate.chain_subjects.len(), 1);
    assert_eq!(certificate.chain_subjects[0], certificate.subject.clone().unwrap_or_default());
    assert!(certificate.chain_diagnostics.is_empty());

    let status_output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(
        status_output.status.success(),
        "status command should succeed: {}",
        render_output(&status_output)
    );
    let status_stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(status_stdout.contains("kind=status_tls_certificate"));
    assert!(status_stdout.contains("kind=status_tls_vhost_binding"));
    assert!(status_stdout.contains("kind=status_tls_sni_binding"));
    assert!(status_stdout.contains("sha256="));
    assert!(status_stdout.contains("san_dns_names=localhost"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn status_and_upstreams_commands_report_upstream_tls_diagnostics() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-upstream-tls", |temp_dir| {
        let ca = generate_ca_cert("admin-upstream-ca");
        let crl = generate_crl(&ca, 42);
        let client_identity = generate_cert("upstream-client");
        let ca_path = temp_dir.join("upstream-ca.pem");
        let crl_path = temp_dir.join("upstream.crl.pem");
        let client_cert_path = temp_dir.join("upstream-client.crt");
        let client_key_path = temp_dir.join("upstream-client.key");
        std::fs::write(&ca_path, ca.cert.pem()).expect("upstream CA should be written");
        std::fs::write(&crl_path, crl.pem().expect("CRL PEM should encode"))
            .expect("upstream CRL should be written");
        std::fs::write(&client_cert_path, client_identity.cert.pem())
            .expect("upstream client cert should be written");
        std::fs::write(&client_key_path, client_identity.key_pair.serialize_pem())
            .expect("upstream client key should be written");

        format!(
            "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: \"https://127.0.0.1:9443\",\n                ),\n            ],\n            tls: Some(UpstreamTlsConfig(\n                verify: CustomCa(\n                    ca_cert_path: {:?},\n                ),\n                versions: Some([Tls12, Tls13]),\n                verify_depth: Some(2),\n                crl_path: Some({:?}),\n                client_cert_path: Some({:?}),\n                client_key_path: Some({:?}),\n            )),\n            protocol: Http2,\n            server_name: Some(false),\n            server_name_override: Some(\"api.internal.example\"),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
            listen_addr.to_string(),
            ca_path.display().to_string(),
            crl_path.display().to_string(),
            client_cert_path.display().to_string(),
            client_key_path.display().to_string(),
            ready_route = READY_ROUTE_CONFIG,
        )
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetStatus)
        .expect("admin socket should return status");
    let AdminResponse::Status(status) = response else {
        panic!("admin socket should return status");
    };
    assert_eq!(status.upstream_tls.len(), 1);
    let upstream_tls = &status.upstream_tls[0];
    assert_eq!(upstream_tls.upstream_name, "backend");
    assert_eq!(upstream_tls.protocol, "http2");
    assert_eq!(upstream_tls.verify_mode, "custom_ca");
    assert_eq!(
        upstream_tls.tls_versions.as_deref(),
        Some(&["TLS1.2".to_string(), "TLS1.3".to_string()][..])
    );
    assert!(!upstream_tls.server_name_enabled);
    assert_eq!(upstream_tls.server_name_override.as_deref(), Some("api.internal.example"));
    assert_eq!(upstream_tls.verify_depth, Some(2));
    assert!(upstream_tls.crl_configured);
    assert!(upstream_tls.client_identity_configured);

    let response =
        query_admin_socket(&socket_path, AdminRequest::GetUpstreamStats { window_secs: None })
            .expect("admin socket should return upstream stats");
    let AdminResponse::UpstreamStats(upstreams) = response else {
        panic!("admin socket should return upstream stats");
    };
    assert_eq!(upstreams.len(), 1);
    assert_eq!(upstreams[0].tls, status.upstream_tls[0]);

    let status_output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(
        status_output.status.success(),
        "status command should succeed: {}",
        render_output(&status_output)
    );
    let status_stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(status_stdout.contains(
        "kind=status_upstream_tls upstream=backend protocol=http2 verify_mode=custom_ca"
    ));
    assert!(status_stdout.contains("tls_versions=TLS1.2,TLS1.3"));
    assert!(status_stdout.contains("server_name_enabled=false"));
    assert!(status_stdout.contains("server_name_override=api.internal.example"));
    assert!(status_stdout.contains("verify_depth=2"));
    assert!(status_stdout.contains("crl_configured=true"));
    assert!(status_stdout.contains("client_identity_configured=true"));

    let upstreams_output =
        run_rginx(["--config", server.config_path().to_str().unwrap(), "upstreams"]);
    assert!(
        upstreams_output.status.success(),
        "upstreams command should succeed: {}",
        render_output(&upstreams_output)
    );
    let upstreams_stdout = String::from_utf8_lossy(&upstreams_output.stdout);
    assert!(upstreams_stdout.contains("kind=upstream_stats upstream=backend"));
    assert!(upstreams_stdout.contains("tls_protocol=http2"));
    assert!(upstreams_stdout.contains("tls_verify_mode=custom_ca"));
    assert!(upstreams_stdout.contains("tls_versions=TLS1.2,TLS1.3"));
    assert!(upstreams_stdout.contains("tls_server_name_enabled=false"));
    assert!(upstreams_stdout.contains("tls_server_name_override=api.internal.example"));
    assert!(upstreams_stdout.contains("tls_verify_depth=2"));
    assert!(upstreams_stdout.contains("tls_crl_configured=true"));
    assert!(upstreams_stdout.contains("tls_client_identity_configured=true"));
    assert!(upstreams_stdout.contains("tls_failures_unknown_ca_total=0"));
    assert!(upstreams_stdout.contains("tls_failures_bad_certificate_total=0"));
    assert!(upstreams_stdout.contains("tls_failures_certificate_revoked_total=0"));
    assert!(upstreams_stdout.contains("tls_failures_verify_depth_exceeded_total=0"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn snapshot_version_command_reports_current_snapshot_version() {
    let listen_addr = reserve_loopback_addr();
    let mut server =
        ServerHarness::spawn("rginx-admin-snapshot-version", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetSnapshotVersion)
        .expect("admin socket should return snapshot version");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };

    let output =
        run_rginx(["--config", server.config_path().to_str().unwrap(), "snapshot-version"]);
    assert!(
        output.status.success(),
        "snapshot-version command should succeed: {}",
        render_output(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&format!("snapshot_version={}", snapshot.snapshot_version)));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn wait_command_returns_new_snapshot_version_after_local_activity() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-wait", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetSnapshotVersion)
        .expect("admin socket should return snapshot version");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };
    let since_version = snapshot.snapshot_version;

    let (status, body) =
        fetch_text_response(listen_addr, "/").expect("root request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "ok\n");

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::WaitForSnapshotChange { since_version, timeout_ms: Some(500) },
    )
    .expect("admin socket should wait for snapshot change");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };
    assert!(snapshot.snapshot_version > since_version);

    let since_version_arg = since_version.to_string();
    let output = run_rginx([
        "--config",
        server.config_path().to_str().unwrap(),
        "wait",
        "--since-version",
        &since_version_arg,
        "--timeout-ms",
        "500",
    ]);
    assert!(output.status.success(), "wait command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let waited_version = stdout
        .lines()
        .find_map(|line| line.strip_prefix("snapshot_version="))
        .and_then(|value| value.parse::<u64>().ok())
        .expect("wait command should print snapshot version");
    assert!(waited_version >= snapshot.snapshot_version);

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn delta_command_reports_changed_modules_since_version() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-delta", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetSnapshotVersion)
        .expect("admin socket should return snapshot version");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };
    let since_version = snapshot.snapshot_version;

    let (status, body) =
        fetch_text_response(listen_addr, "/").expect("root request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "ok\n");

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::GetDelta { since_version, include: None, window_secs: None },
    )
    .expect("admin socket should return delta");
    let AdminResponse::Delta(delta) = response else {
        panic!("admin socket should return delta");
    };
    assert_eq!(delta.schema_version, 2);
    assert_eq!(delta.since_version, since_version);
    assert!(delta.current_snapshot_version > since_version);
    assert_eq!(delta.included_modules, rginx_http::SnapshotModule::all());
    assert_eq!(delta.status_changed, Some(true));
    assert_eq!(delta.counters_changed, Some(true));
    assert_eq!(delta.traffic_changed, Some(true));
    assert_eq!(delta.traffic_recent_changed, None);
    assert_eq!(delta.peer_health_changed, Some(false));
    assert_eq!(delta.upstreams_changed, Some(false));
    assert_eq!(delta.upstreams_recent_changed, None);
    assert_eq!(delta.changed_listener_ids, Some(vec!["default".to_string()]));
    assert_eq!(delta.changed_vhost_ids, Some(vec!["server".to_string()]));
    let changed_route_ids =
        delta.changed_route_ids.as_ref().expect("delta should report changed routes");
    assert!(
        changed_route_ids.iter().any(|route| route == "server/routes[1]|exact:/"),
        "delta should include the business route change: {changed_route_ids:?}"
    );
    assert!(
        changed_route_ids.iter().all(|route| {
            route == "server/routes[0]|exact:/-/ready" || route == "server/routes[1]|exact:/"
        }),
        "delta should only report root and optional ready route changes: {changed_route_ids:?}"
    );
    assert_eq!(delta.recent_window_secs, None);
    assert_eq!(delta.changed_recent_listener_ids, None);
    assert_eq!(delta.changed_recent_vhost_ids, None);
    assert_eq!(delta.changed_recent_route_ids, None);
    assert_eq!(delta.changed_peer_health_upstream_names, Some(Vec::new()));
    assert_eq!(delta.changed_upstream_names, Some(Vec::new()));
    assert_eq!(delta.changed_recent_upstream_names, None);

    let since_version_arg = since_version.to_string();
    let output = run_rginx([
        "--config",
        server.config_path().to_str().unwrap(),
        "delta",
        "--since-version",
        &since_version_arg,
    ]);
    assert!(output.status.success(), "delta command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let delta: serde_json::Value =
        serde_json::from_str(&stdout).expect("delta command should print valid JSON");
    assert_eq!(delta["schema_version"], serde_json::Value::from(2));
    assert_eq!(delta["since_version"], serde_json::Value::from(since_version));
    assert_eq!(delta["status_changed"], serde_json::Value::from(true));
    assert_eq!(delta["counters_changed"], serde_json::Value::from(true));
    assert_eq!(delta["traffic_changed"], serde_json::Value::from(true));
    assert!(delta.get("traffic_recent_changed").is_none());
    assert_eq!(delta["peer_health_changed"], serde_json::Value::from(false));
    assert_eq!(delta["upstreams_changed"], serde_json::Value::from(false));
    assert!(delta.get("upstreams_recent_changed").is_none());
    assert_eq!(delta["changed_listener_ids"], serde_json::json!(["default"]));
    assert_eq!(delta["changed_vhost_ids"], serde_json::json!(["server"]));
    let changed_route_ids =
        delta["changed_route_ids"].as_array().expect("delta JSON should include changed_route_ids");
    assert!(
        changed_route_ids.iter().any(|route| route == "server/routes[1]|exact:/"),
        "delta JSON should include the business route change: {changed_route_ids:?}"
    );
    assert!(
        changed_route_ids.iter().all(|route| {
            route == "server/routes[0]|exact:/-/ready" || route == "server/routes[1]|exact:/"
        }),
        "delta JSON should only report root and optional ready route changes: {changed_route_ids:?}"
    );
    assert!(delta.get("recent_window_secs").is_none());
    assert!(delta.get("changed_recent_listener_ids").is_none());
    assert!(delta.get("changed_recent_vhost_ids").is_none());
    assert!(delta.get("changed_recent_route_ids").is_none());
    assert_eq!(delta["changed_peer_health_upstream_names"], serde_json::json!([]));
    assert_eq!(delta["changed_upstream_names"], serde_json::json!([]));
    assert!(delta.get("changed_recent_upstream_names").is_none());

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn delta_command_reports_peer_health_changes_for_proxy_activity() {
    let listen_addr = reserve_loopback_addr();
    let upstream_addr = spawn_response_server("delta upstream ok\n");
    let mut server = ServerHarness::spawn("rginx-admin-delta-peer-health", |_| {
        proxy_config(listen_addr, upstream_addr)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetSnapshotVersion)
        .expect("admin socket should return snapshot version");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };
    let since_version = snapshot.snapshot_version;

    let (status, body) =
        fetch_text_response(listen_addr, "/api/demo").expect("proxy request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "delta upstream ok\n");

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::GetDelta { since_version, include: None, window_secs: None },
    )
    .expect("admin socket should return delta");
    let AdminResponse::Delta(delta) = response else {
        panic!("admin socket should return delta");
    };
    assert_eq!(delta.peer_health_changed, Some(true));
    assert_eq!(delta.upstreams_changed, Some(true));
    assert_eq!(delta.changed_peer_health_upstream_names, Some(vec!["backend".to_string()]));
    assert_eq!(delta.changed_upstream_names, Some(vec!["backend".to_string()]));
    assert_eq!(delta.changed_recent_upstream_names, None);

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn delta_command_can_request_recent_window_summary() {
    let listen_addr = reserve_loopback_addr();
    let upstream_addr = spawn_response_server("delta recent upstream ok\n");
    let mut server = ServerHarness::spawn("rginx-admin-delta-recent", |_| {
        proxy_config(listen_addr, upstream_addr)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(&socket_path, AdminRequest::GetSnapshotVersion)
        .expect("admin socket should return snapshot version");
    let AdminResponse::SnapshotVersion(snapshot) = response else {
        panic!("admin socket should return snapshot version");
    };
    let since_version = snapshot.snapshot_version;

    let (status, body) =
        fetch_text_response(listen_addr, "/api/demo").expect("proxy request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "delta recent upstream ok\n");

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::GetDelta {
            since_version,
            include: Some(vec![
                rginx_http::SnapshotModule::Traffic,
                rginx_http::SnapshotModule::Upstreams,
            ]),
            window_secs: Some(300),
        },
    )
    .expect("admin socket should return delta");
    let AdminResponse::Delta(delta) = response else {
        panic!("admin socket should return delta");
    };
    assert_eq!(delta.recent_window_secs, Some(300));
    assert_eq!(delta.traffic_changed, Some(true));
    assert_eq!(delta.traffic_recent_changed, Some(true));
    assert_eq!(delta.upstreams_changed, Some(true));
    assert_eq!(delta.upstreams_recent_changed, Some(true));
    assert_eq!(delta.changed_recent_listener_ids, Some(vec!["default".to_string()]));
    assert_eq!(delta.changed_recent_upstream_names, Some(vec!["backend".to_string()]));

    let since_version_arg = since_version.to_string();
    let output = run_rginx([
        "--config",
        server.config_path().to_str().unwrap(),
        "delta",
        "--since-version",
        &since_version_arg,
        "--include",
        "traffic",
        "--include",
        "upstreams",
        "--window-secs",
        "300",
    ]);
    assert!(output.status.success(), "delta command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let delta: serde_json::Value =
        serde_json::from_str(&stdout).expect("delta command should print valid JSON");
    assert_eq!(delta["recent_window_secs"], serde_json::Value::from(300));
    assert_eq!(delta["traffic_recent_changed"], serde_json::Value::from(true));
    assert_eq!(delta["upstreams_recent_changed"], serde_json::Value::from(true));
    assert_eq!(delta["changed_recent_listener_ids"], serde_json::json!(["default"]));
    assert_eq!(delta["changed_recent_upstream_names"], serde_json::json!(["backend"]));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn snapshot_command_can_filter_modules() {
    let listen_addr = reserve_loopback_addr();
    let upstream_addr = spawn_response_server("filtered snapshot upstream ok\n");
    let mut server = ServerHarness::spawn("rginx-admin-snapshot-filtered", |_| {
        proxy_config(listen_addr, upstream_addr)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = query_admin_socket(
        &socket_path,
        AdminRequest::GetSnapshot {
            include: Some(vec![
                rginx_http::SnapshotModule::Traffic,
                rginx_http::SnapshotModule::Upstreams,
            ]),
            window_secs: Some(300),
        },
    )
    .expect("admin socket should return filtered snapshot");
    let AdminResponse::Snapshot(snapshot) = response else {
        panic!("admin socket should return filtered snapshot");
    };
    assert_eq!(
        snapshot.included_modules,
        vec![rginx_http::SnapshotModule::Traffic, rginx_http::SnapshotModule::Upstreams,]
    );
    assert!(snapshot.status.is_none());
    assert!(snapshot.counters.is_none());
    assert!(snapshot.peer_health.is_none());
    assert!(snapshot.traffic.is_some());
    assert!(snapshot.upstreams.is_some());
    assert_eq!(
        snapshot
            .traffic
            .as_ref()
            .and_then(|traffic| traffic.listeners.first())
            .and_then(|listener| listener.recent_window.as_ref())
            .map(|recent| recent.window_secs),
        Some(300)
    );
    assert_eq!(
        snapshot
            .upstreams
            .as_ref()
            .and_then(|upstreams| upstreams.first())
            .and_then(|upstream| upstream.recent_window.as_ref())
            .map(|recent| recent.window_secs),
        Some(300)
    );

    let output = run_rginx([
        "--config",
        server.config_path().to_str().unwrap(),
        "snapshot",
        "--include",
        "traffic",
        "--include",
        "upstreams",
        "--window-secs",
        "300",
    ]);
    assert!(output.status.success(), "snapshot command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let snapshot: serde_json::Value =
        serde_json::from_str(&stdout).expect("snapshot command should print valid JSON");
    assert_eq!(snapshot["included_modules"], serde_json::json!(["traffic", "upstreams"]));
    assert!(snapshot.get("status").is_none());
    assert!(snapshot.get("counters").is_none());
    assert!(snapshot.get("peer_health").is_none());
    assert!(snapshot.get("traffic").is_some());
    assert!(snapshot.get("upstreams").is_some());
    assert_eq!(
        snapshot["traffic"]["listeners"][0]["recent_window"]["window_secs"],
        serde_json::Value::from(300)
    );
    assert_eq!(
        snapshot["upstreams"][0]["recent_window"]["window_secs"],
        serde_json::Value::from(300)
    );

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
    assert!(stdout.contains("kind=status"));
    assert!(stdout.contains("revision=0"));
    assert!(stdout.contains(&format!("listen={listen_addr}")));
    assert!(stdout.contains("tls_listeners=1"));
    assert!(stdout.contains("tls_certificates=0"));
    assert!(stdout.contains("tls_expiring_certificates=0"));
    assert!(stdout.contains("active_connections=0"));
    assert!(stdout.contains("mtls_listeners=0"));
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
    assert!(stdout.contains("kind=counters"));
    assert!(stdout.contains("downstream_mtls_authenticated_requests_total=0"));
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
fn traffic_command_reports_listener_vhost_and_route_counters() {
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-admin-traffic", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let (status, body) =
        fetch_text_response(listen_addr, "/").expect("root request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "ok\n");
    let (status, body) =
        fetch_text_response(listen_addr, "/missing").expect("missing request should respond");
    assert_eq!(status, 404);
    assert_eq!(body, "route not found\n");

    let response =
        query_admin_socket(&socket_path, AdminRequest::GetTrafficStats { window_secs: Some(300) })
            .expect("admin socket should return traffic stats");
    let AdminResponse::TrafficStats(traffic) = response else {
        panic!("admin socket should return traffic stats");
    };
    assert_eq!(traffic.listeners.len(), 1);
    assert_eq!(traffic.listeners[0].listener_id, "default");
    assert!(traffic.listeners[0].downstream_requests >= 3);
    assert!(traffic.listeners[0].unmatched_requests_total >= 1);
    assert!(traffic.listeners[0].downstream_responses_2xx >= 2);
    assert!(traffic.listeners[0].downstream_responses_4xx >= 1);
    assert_eq!(traffic.vhosts.len(), 1);
    assert_eq!(traffic.vhosts[0].vhost_id, "server");
    assert!(traffic.vhosts[0].downstream_requests >= 3);
    assert!(traffic.vhosts[0].unmatched_requests_total >= 1);
    let route = traffic
        .routes
        .iter()
        .find(|route| route.route_id.ends_with("|exact:/"))
        .expect("root route should be present in traffic stats");
    assert_eq!(route.vhost_id, "server");
    assert_eq!(route.downstream_requests, 1);
    assert_eq!(route.downstream_responses_2xx, 1);
    assert_eq!(route.recent_60s.window_secs, 60);
    assert_eq!(route.recent_60s.downstream_requests_total, 1);
    assert_eq!(route.recent_60s.downstream_responses_total, 1);
    assert_eq!(route.recent_window.as_ref().map(|recent| recent.window_secs), Some(300));
    assert_eq!(
        route.recent_window.as_ref().map(|recent| recent.downstream_requests_total),
        Some(1)
    );

    let output = run_rginx([
        "--config",
        server.config_path().to_str().unwrap(),
        "traffic",
        "--window-secs",
        "300",
    ]);
    assert!(output.status.success(), "traffic command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind=traffic_listener"));
    assert!(stdout.contains("kind=traffic_vhost"));
    assert!(stdout.contains("kind=traffic_route"));
    assert!(stdout.contains("kind=traffic_listener_recent_window"));
    assert!(stdout.contains("kind=traffic_route_recent_window"));
    assert!(stdout.contains("listener=default"));
    assert!(stdout.contains("vhost=server"));
    assert!(stdout.contains("route=server/routes"));
    assert!(stdout.contains("unmatched_requests_total=1"));
    assert!(stdout.contains("downstream_requests_total=1"));
    assert!(stdout.contains("recent_60s_window_secs=60"));
    assert!(stdout.contains("recent_60s_downstream_requests_total=1"));
    assert!(stdout.contains("recent_window_secs=300"));
    assert!(stdout.contains("recent_window_downstream_requests_total=1"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn traffic_command_reports_grpc_request_and_status_counters() {
    let listen_addr = reserve_loopback_addr();
    let mut server =
        ServerHarness::spawn("rginx-admin-traffic-grpc", |_| return_config(listen_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let response = send_raw_request(
        listen_addr,
        &format!(
            "POST /grpc.health.v1.Health/Check HTTP/1.1\r\nHost: {listen_addr}\r\nContent-Type: application/grpc\r\nTE: trailers\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("grpc-like request should succeed");
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("grpc-status: 12"));

    let response =
        query_admin_socket(&socket_path, AdminRequest::GetTrafficStats { window_secs: None })
            .expect("admin socket should return traffic stats");
    let AdminResponse::TrafficStats(traffic) = response else {
        panic!("admin socket should return traffic stats");
    };
    assert_eq!(traffic.listeners.len(), 1);
    assert!(traffic.listeners[0].grpc.requests_total >= 1);
    assert!(traffic.listeners[0].grpc.protocol_grpc_total >= 1);
    assert!(traffic.listeners[0].grpc.status_12_total >= 1);
    assert!(traffic.vhosts[0].grpc.requests_total >= 1);
    assert!(traffic.vhosts[0].grpc.status_12_total >= 1);

    let output = run_rginx(["--config", server.config_path().to_str().unwrap(), "traffic"]);
    assert!(output.status.success(), "traffic command should succeed: {}", render_output(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind=traffic_listener"));
    assert!(stdout.contains("grpc_requests_total=1"));
    assert!(stdout.contains("grpc_protocol_grpc_total=1"));
    assert!(stdout.contains("grpc_status_12_total=1"));

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
    assert!(stdout.contains("kind=peer_health_upstream"));
    assert!(stdout.contains("kind=peer_health_peer"));
    assert!(stdout.contains("upstream=backend"));
    assert!(stdout.contains(&format!("peer=http://{upstream_addr}")));
    assert!(stdout.contains("available=true"));
    assert!(stdout.contains("backup=false"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn upstreams_command_reports_upstream_request_counters() {
    let listen_addr = reserve_loopback_addr();
    let upstream_addr = spawn_response_server("admin upstream ok\n");
    let mut server =
        ServerHarness::spawn("rginx-admin-upstreams", |_| proxy_config(listen_addr, upstream_addr));
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));
    let socket_path = admin_socket_path_for_config(server.config_path());
    wait_for_admin_socket(&socket_path, Duration::from_secs(5));

    let (status, body) =
        fetch_text_response(listen_addr, "/api/demo").expect("proxy request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "admin upstream ok\n");

    let response =
        query_admin_socket(&socket_path, AdminRequest::GetUpstreamStats { window_secs: Some(300) })
            .expect("admin socket should return upstream stats");
    let AdminResponse::UpstreamStats(upstreams) = response else {
        panic!("admin socket should return upstream stats");
    };
    assert_eq!(upstreams.len(), 1);
    assert_eq!(upstreams[0].upstream_name, "backend");
    assert_eq!(upstreams[0].downstream_requests_total, 1);
    assert_eq!(upstreams[0].peer_attempts_total, 1);
    assert_eq!(upstreams[0].peer_successes_total, 1);
    assert_eq!(upstreams[0].peer_failures_total, 0);
    assert_eq!(upstreams[0].peer_timeouts_total, 0);
    assert_eq!(upstreams[0].failovers_total, 0);
    assert_eq!(upstreams[0].completed_responses_total, 1);
    assert_eq!(upstreams[0].bad_gateway_responses_total, 0);
    assert_eq!(upstreams[0].gateway_timeout_responses_total, 0);
    assert_eq!(upstreams[0].bad_request_responses_total, 0);
    assert_eq!(upstreams[0].payload_too_large_responses_total, 0);
    assert_eq!(upstreams[0].unsupported_media_type_responses_total, 0);
    assert_eq!(upstreams[0].no_healthy_peers_total, 0);
    assert_eq!(upstreams[0].recent_60s.window_secs, 60);
    assert_eq!(upstreams[0].recent_60s.downstream_requests_total, 1);
    assert_eq!(upstreams[0].recent_60s.peer_attempts_total, 1);
    assert_eq!(upstreams[0].recent_60s.completed_responses_total, 1);
    assert_eq!(upstreams[0].recent_window.as_ref().map(|recent| recent.window_secs), Some(300));
    assert_eq!(
        upstreams[0].recent_window.as_ref().map(|recent| recent.downstream_requests_total),
        Some(1)
    );
    assert_eq!(upstreams[0].peers.len(), 1);
    assert_eq!(upstreams[0].peers[0].peer_url, format!("http://{upstream_addr}"));
    assert_eq!(upstreams[0].peers[0].attempts_total, 1);
    assert_eq!(upstreams[0].peers[0].successes_total, 1);

    let output = run_rginx([
        "--config",
        server.config_path().to_str().unwrap(),
        "upstreams",
        "--window-secs",
        "300",
    ]);
    assert!(
        output.status.success(),
        "upstreams command should succeed: {}",
        render_output(&output)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("kind=upstream_stats"));
    assert!(stdout.contains("kind=upstream_stats_peer"));
    assert!(stdout.contains("kind=upstream_stats_recent_window"));
    assert!(stdout.contains("upstream=backend"));
    assert!(stdout.contains("downstream_requests_total=1"));
    assert!(stdout.contains("peer_attempts_total=1"));
    assert!(stdout.contains("peer_successes_total=1"));
    assert!(stdout.contains("completed_responses_total=1"));
    assert!(stdout.contains("recent_60s_window_secs=60"));
    assert!(stdout.contains("recent_60s_downstream_requests_total=1"));
    assert!(stdout.contains("recent_window_secs=300"));
    assert!(stdout.contains("recent_window_downstream_requests_total=1"));
    assert!(stdout.contains(&format!("peer=http://{upstream_addr}")));

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

fn generate_cert(hostname: &str) -> CertifiedKey {
    let cert = rcgen::generate_simple_self_signed(vec![hostname.to_string()])
        .expect("self-signed certificate should generate");
    CertifiedKey { cert: cert.cert, key_pair: cert.key_pair }
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

fn generate_crl(issuer: &CertifiedKey, revoked_serial: u64) -> CertificateRevocationList {
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
    .signed_by(&issuer.cert, &issuer.key_pair)
    .expect("CRL should be signed")
}
