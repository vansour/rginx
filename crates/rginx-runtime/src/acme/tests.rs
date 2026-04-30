use std::collections::HashMap;
use std::fs;
use std::io::{Error as IoError, ErrorKind};
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use super::challenge::is_transient_accept_error;
use super::storage::parent_directory;
use rginx_core::{
    AcmeChallengeType, AcmeSettings, ConfigSnapshot, Listener, ManagedCertificateSpec,
    RuntimeSettings, Server, VirtualHost,
};
use rginx_http::TlsCertificateStatusSnapshot;

use super::storage::write_certificate_pair;
use super::types::{http01_listener_addrs, plan_reconcile};

fn test_config(listeners: Vec<Listener>) -> ConfigSnapshot {
    ConfigSnapshot {
        runtime: RuntimeSettings {
            shutdown_timeout: Duration::from_secs(1),
            worker_threads: None,
            accept_workers: 1,
        },
        acme: Some(AcmeSettings {
            directory_url: "https://acme-staging-v02.api.letsencrypt.org/directory".to_string(),
            contacts: Vec::new(),
            state_dir: PathBuf::from("/tmp/rginx-acme-tests"),
            renew_before: Duration::from_secs(30 * 86_400),
            poll_interval: Duration::from_secs(3600),
        }),
        managed_certificates: Vec::new(),
        listeners,
        default_vhost: VirtualHost {
            id: "server".to_string(),
            server_names: Vec::new(),
            routes: Vec::new(),
            tls: None,
        },
        vhosts: Vec::new(),
        cache_zones: HashMap::new(),
        upstreams: HashMap::new(),
    }
}

fn test_listener(listen_addr: &str, tls_enabled: bool) -> Listener {
    Listener {
        id: listen_addr.to_string(),
        name: listen_addr.to_string(),
        server: Server {
            listen_addr: listen_addr.parse().unwrap(),
            server_header: rginx_core::default_server_header(),
            default_certificate: None,
            trusted_proxies: Vec::new(),
            client_ip_header: None,
            keep_alive: true,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout: None,
            request_body_read_timeout: None,
            response_write_timeout: None,
            access_log_format: None,
            tls: None,
        },
        tls_termination_enabled: tls_enabled,
        proxy_protocol_enabled: false,
        http3: None,
    }
}

fn managed_spec() -> ManagedCertificateSpec {
    ManagedCertificateSpec {
        scope: "servers[0]".to_string(),
        domains: vec!["api.example.com".to_string(), "www.example.com".to_string()],
        cert_path: PathBuf::from("/tmp/api.example.com.crt"),
        key_path: PathBuf::from("/tmp/api.example.com.key"),
        challenge: AcmeChallengeType::Http01,
    }
}

#[test]
fn http01_listener_addrs_only_returns_plain_http_port_80_bindings() {
    let config = test_config(vec![
        test_listener("127.0.0.1:80", false),
        test_listener("127.0.0.1:443", true),
        test_listener("127.0.0.1:8080", false),
    ]);

    assert_eq!(http01_listener_addrs(&config), vec!["127.0.0.1:80".parse().unwrap()]);
}

#[test]
fn transient_accept_errors_are_retried() {
    assert!(is_transient_accept_error(&IoError::from(ErrorKind::ConnectionAborted)));
    assert!(is_transient_accept_error(&IoError::from(ErrorKind::OutOfMemory)));
}

#[test]
fn permanent_accept_errors_stop_the_listener() {
    assert!(!is_transient_accept_error(&IoError::from(ErrorKind::AddrInUse)));
}

#[test]
fn bare_relative_paths_use_current_directory_as_parent() {
    assert_eq!(parent_directory(Path::new("issued.crt")), Path::new("."));
}

#[test]
fn plan_reconcile_detects_san_mismatch() {
    let settings = test_config(Vec::new()).acme.unwrap();
    let temp_dir = tempfile::tempdir().expect("tempdir should build");
    let spec = ManagedCertificateSpec {
        cert_path: temp_dir.path().join("issued.crt"),
        key_path: temp_dir.path().join("issued.key"),
        ..managed_spec()
    };
    fs::write(&spec.key_path, b"private-key").expect("private key should be written");
    let status = TlsCertificateStatusSnapshot {
        scope: spec.scope.clone(),
        cert_path: spec.cert_path.clone(),
        server_names: spec.domains.clone(),
        subject: None,
        issuer: None,
        serial_number: None,
        san_dns_names: vec!["api.example.com".to_string()],
        fingerprint_sha256: Some("fingerprint".to_string()),
        subject_key_identifier: None,
        authority_key_identifier: None,
        is_ca: Some(false),
        path_len_constraint: None,
        key_usage: None,
        extended_key_usage: Vec::new(),
        not_before_unix_ms: None,
        not_after_unix_ms: None,
        expires_in_days: Some(45),
        chain_length: 1,
        chain_subjects: Vec::new(),
        chain_diagnostics: Vec::new(),
        selected_as_default_for_listeners: Vec::new(),
        ocsp_staple_configured: false,
        additional_certificate_count: 0,
    };

    let plan = plan_reconcile(&spec, Some(&status), &settings)
        .expect("SAN mismatch should trigger reconcile");
    assert!(plan.describe().contains("SAN mismatch"));
}

#[test]
fn plan_reconcile_skips_healthy_certificate() {
    let settings = test_config(Vec::new()).acme.unwrap();
    let temp_dir = tempfile::tempdir().expect("tempdir should build");
    let spec = ManagedCertificateSpec {
        cert_path: temp_dir.path().join("issued.crt"),
        key_path: temp_dir.path().join("issued.key"),
        ..managed_spec()
    };
    fs::write(&spec.key_path, b"private-key").expect("private key should be written");
    let status = TlsCertificateStatusSnapshot {
        scope: spec.scope.clone(),
        cert_path: spec.cert_path.clone(),
        server_names: spec.domains.clone(),
        subject: None,
        issuer: None,
        serial_number: None,
        san_dns_names: spec.domains.clone(),
        fingerprint_sha256: Some("fingerprint".to_string()),
        subject_key_identifier: None,
        authority_key_identifier: None,
        is_ca: Some(false),
        path_len_constraint: None,
        key_usage: None,
        extended_key_usage: Vec::new(),
        not_before_unix_ms: None,
        not_after_unix_ms: None,
        expires_in_days: Some(90),
        chain_length: 1,
        chain_subjects: Vec::new(),
        chain_diagnostics: Vec::new(),
        selected_as_default_for_listeners: Vec::new(),
        ocsp_staple_configured: false,
        additional_certificate_count: 0,
    };

    assert!(plan_reconcile(&spec, Some(&status), &settings).is_none());
}

#[test]
fn plan_reconcile_detects_missing_private_key() {
    let settings = test_config(Vec::new()).acme.unwrap();
    let temp_dir = tempfile::tempdir().expect("tempdir should build");
    let spec = ManagedCertificateSpec {
        cert_path: temp_dir.path().join("issued.crt"),
        key_path: temp_dir.path().join("issued.key"),
        ..managed_spec()
    };
    fs::write(&spec.cert_path, b"certificate-chain").expect("certificate should be written");
    let status = TlsCertificateStatusSnapshot {
        scope: spec.scope.clone(),
        cert_path: spec.cert_path.clone(),
        server_names: spec.domains.clone(),
        subject: None,
        issuer: None,
        serial_number: None,
        san_dns_names: spec.domains.clone(),
        fingerprint_sha256: Some("fingerprint".to_string()),
        subject_key_identifier: None,
        authority_key_identifier: None,
        is_ca: Some(false),
        path_len_constraint: None,
        key_usage: None,
        extended_key_usage: Vec::new(),
        not_before_unix_ms: None,
        not_after_unix_ms: None,
        expires_in_days: Some(90),
        chain_length: 1,
        chain_subjects: Vec::new(),
        chain_diagnostics: Vec::new(),
        selected_as_default_for_listeners: Vec::new(),
        ocsp_staple_configured: false,
        additional_certificate_count: 0,
    };

    let plan = plan_reconcile(&spec, Some(&status), &settings)
        .expect("missing private key should trigger reconcile");
    assert!(plan.describe().contains("private key file is missing"));
}

#[test]
fn write_certificate_pair_persists_both_outputs() {
    let temp_dir = tempfile::tempdir().expect("tempdir should build");
    let spec = ManagedCertificateSpec {
        scope: "servers[0]".to_string(),
        domains: vec!["api.example.com".to_string()],
        cert_path: temp_dir.path().join("issued.crt"),
        key_path: temp_dir.path().join("issued.key"),
        challenge: AcmeChallengeType::Http01,
    };

    write_certificate_pair(&spec, "certificate-chain", "private-key")
        .expect("certificate material should write");

    assert_eq!(
        std::fs::read_to_string(&spec.cert_path).expect("certificate should read"),
        "certificate-chain"
    );
    assert_eq!(
        std::fs::read_to_string(&spec.key_path).expect("private key should read"),
        "private-key"
    );
}

#[test]
fn write_certificate_pair_rolls_back_certificate_when_key_write_fails() {
    let temp_dir = tempfile::tempdir().expect("tempdir should build");
    let blocked_parent = temp_dir.path().join("blocked-parent");
    fs::write(&blocked_parent, b"not-a-directory").expect("blocking file should be written");

    let spec = ManagedCertificateSpec {
        scope: "servers[0]".to_string(),
        domains: vec!["api.example.com".to_string()],
        cert_path: temp_dir.path().join("issued.crt"),
        key_path: blocked_parent.join("issued.key"),
        challenge: AcmeChallengeType::Http01,
    };
    fs::write(&spec.cert_path, b"old-certificate").expect("existing certificate should be written");

    let error = write_certificate_pair(&spec, "new-certificate", "private-key")
        .expect_err("key write should fail");

    assert!(
        error.to_string().contains("Not a directory")
            || error.to_string().contains("not a directory")
            || error.to_string().contains("File exists")
    );
    assert_eq!(
        fs::read_to_string(&spec.cert_path).expect("certificate should read"),
        "old-certificate"
    );
    assert!(!spec.key_path.exists());
}
