use std::path::{Path, PathBuf};

use super::{AdminResponse, INSTALLED_ADMIN_SOCKET_PATH, admin_socket_path_for_config};

#[test]
fn installed_config_uses_run_admin_socket() {
    assert_eq!(
        admin_socket_path_for_config(Path::new("/etc/rginx/rginx.ron")),
        PathBuf::from(INSTALLED_ADMIN_SOCKET_PATH)
    );
}

#[test]
fn custom_config_uses_neighbor_admin_socket() {
    assert_eq!(
        admin_socket_path_for_config(Path::new("/tmp/site.ron")),
        PathBuf::from("/tmp/site.admin.sock")
    );
}

#[test]
fn status_response_accepts_older_payload_without_acme_snapshot() {
    let response = serde_json::json!({
        "type": "Status",
        "data": {
            "revision": 1,
            "config_path": null,
            "listeners": [],
            "worker_threads": null,
            "accept_workers": 0,
            "total_vhosts": 0,
            "total_routes": 0,
            "total_upstreams": 0,
            "tls_enabled": false,
            "http3_active_connections": 0,
            "http3_active_request_streams": 0,
            "http3_retry_issued_total": 0,
            "http3_retry_failed_total": 0,
            "http3_request_accept_errors_total": 0,
            "http3_request_resolve_errors_total": 0,
            "http3_request_body_stream_errors_total": 0,
            "http3_response_stream_errors_total": 0,
            "http3_early_data_enabled_listeners": 0,
            "http3_early_data_accepted_requests": 0,
            "http3_early_data_rejected_requests": 0,
            "tls": {
                "listeners": [],
                "certificates": [],
                "ocsp": [],
                "vhost_bindings": [],
                "sni_bindings": [],
                "sni_conflicts": [],
                "default_certificate_bindings": [],
                "reload_boundary": {
                    "reloadable_fields": [],
                    "restart_required_fields": []
                },
                "expiring_certificate_count": 0
            },
            "mtls": {
                "configured_listeners": 0,
                "optional_listeners": 0,
                "required_listeners": 0,
                "authenticated_connections": 0,
                "authenticated_requests": 0,
                "anonymous_requests": 0,
                "handshake_failures_total": 0,
                "handshake_failures_missing_client_cert": 0,
                "handshake_failures_unknown_ca": 0,
                "handshake_failures_bad_certificate": 0,
                "handshake_failures_certificate_revoked": 0,
                "handshake_failures_verify_depth_exceeded": 0,
                "handshake_failures_other": 0
            },
            "upstream_tls": [],
            "cache": {
                "zones": []
            },
            "active_connections": 0,
            "reload": {
                "attempts_total": 0,
                "successes_total": 0,
                "failures_total": 0,
                "last_result": null
            }
        }
    });

    let decoded: AdminResponse =
        serde_json::from_value(response).expect("older status payload should still decode");
    let AdminResponse::Status(status) = decoded else {
        panic!("expected status response");
    };

    assert!(!status.acme.enabled);
    assert!(status.acme.directory_url.is_none());
    assert!(status.acme.managed_certificates.is_empty());
}
