use super::render::{
    print_record, render_last_reload, render_optional_string_list, render_reload_active_revision,
    render_reload_rollback_revision, render_reload_tls_certificate_changes,
};
use super::socket::{query_admin_socket, unexpected_admin_response};
use super::*;

pub(super) fn print_admin_status(config_path: &Path) -> anyhow::Result<()> {
    match query_admin_socket(config_path, AdminRequest::GetStatus)? {
        AdminResponse::Status(status) => {
            print_record(
                "status",
                [
                    ("revision", status.revision.to_string()),
                    (
                        "config_path",
                        status
                            .config_path
                            .as_deref()
                            .map(Path::display)
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "-".to_string()),
                    ),
                    ("listen", status.listen_addr.to_string()),
                    (
                        "worker_threads",
                        status
                            .worker_threads
                            .map(|value| value.to_string())
                            .unwrap_or_else(|| "auto".to_string()),
                    ),
                    ("accept_workers", status.accept_workers.to_string()),
                    ("vhosts", status.total_vhosts.to_string()),
                    ("routes", status.total_routes.to_string()),
                    ("upstreams", status.total_upstreams.to_string()),
                    ("tls", if status.tls_enabled { "enabled" } else { "disabled" }.to_string()),
                    ("tls_listeners", status.tls.listeners.len().to_string()),
                    ("tls_certificates", status.tls.certificates.len().to_string()),
                    ("tls_ocsp_entries", status.tls.ocsp.len().to_string()),
                    ("tls_vhost_bindings", status.tls.vhost_bindings.len().to_string()),
                    ("tls_sni_bindings", status.tls.sni_bindings.len().to_string()),
                    ("tls_sni_conflicts", status.tls.sni_conflicts.len().to_string()),
                    (
                        "tls_default_certificate_bindings",
                        status.tls.default_certificate_bindings.len().to_string(),
                    ),
                    (
                        "tls_expiring_certificates",
                        status.tls.expiring_certificate_count.to_string(),
                    ),
                    ("mtls_listeners", status.mtls.configured_listeners.to_string()),
                    ("mtls_optional_listeners", status.mtls.optional_listeners.to_string()),
                    ("mtls_required_listeners", status.mtls.required_listeners.to_string()),
                    (
                        "mtls_authenticated_connections",
                        status.mtls.authenticated_connections.to_string(),
                    ),
                    ("mtls_authenticated_requests", status.mtls.authenticated_requests.to_string()),
                    ("mtls_anonymous_requests", status.mtls.anonymous_requests.to_string()),
                    ("mtls_handshake_failures", status.mtls.handshake_failures_total.to_string()),
                    (
                        "mtls_handshake_failures_missing_client_cert",
                        status.mtls.handshake_failures_missing_client_cert.to_string(),
                    ),
                    (
                        "mtls_handshake_failures_certificate_revoked",
                        status.mtls.handshake_failures_certificate_revoked.to_string(),
                    ),
                    (
                        "mtls_handshake_failures_verify_depth_exceeded",
                        status.mtls.handshake_failures_verify_depth_exceeded.to_string(),
                    ),
                    ("active_connections", status.active_connections.to_string()),
                    ("reload_attempts", status.reload.attempts_total.to_string()),
                    ("reload_successes", status.reload.successes_total.to_string()),
                    ("reload_failures", status.reload.failures_total.to_string()),
                    ("last_reload", render_last_reload(status.reload.last_result.as_ref())),
                    (
                        "last_reload_active_revision",
                        render_reload_active_revision(status.reload.last_result.as_ref()),
                    ),
                    (
                        "last_reload_rollback_revision",
                        render_reload_rollback_revision(status.reload.last_result.as_ref()),
                    ),
                    (
                        "last_reload_tls_certificate_changes",
                        render_reload_tls_certificate_changes(status.reload.last_result.as_ref()),
                    ),
                ],
            );
            for certificate in &status.tls.certificates {
                print_record(
                    "status_tls_certificate",
                    [
                        ("scope", certificate.scope.clone()),
                        ("cert_path", certificate.cert_path.display().to_string()),
                        (
                            "sha256",
                            certificate
                                .fingerprint_sha256
                                .clone()
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        ("subject", certificate.subject.clone().unwrap_or_else(|| "-".to_string())),
                        ("issuer", certificate.issuer.clone().unwrap_or_else(|| "-".to_string())),
                        (
                            "serial",
                            certificate.serial_number.clone().unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "san_dns_names",
                            if certificate.san_dns_names.is_empty() {
                                "-".to_string()
                            } else {
                                certificate.san_dns_names.join(",")
                            },
                        ),
                        (
                            "ski",
                            certificate
                                .subject_key_identifier
                                .clone()
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "aki",
                            certificate
                                .authority_key_identifier
                                .clone()
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "is_ca",
                            certificate
                                .is_ca
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "path_len_constraint",
                            certificate
                                .path_len_constraint
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "key_usage",
                            certificate.key_usage.clone().unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "extended_key_usage",
                            if certificate.extended_key_usage.is_empty() {
                                "-".to_string()
                            } else {
                                certificate.extended_key_usage.join(",")
                            },
                        ),
                        ("chain_length", certificate.chain_length.to_string()),
                        (
                            "chain_diagnostics",
                            if certificate.chain_diagnostics.is_empty() {
                                "-".to_string()
                            } else {
                                certificate.chain_diagnostics.join("|")
                            },
                        ),
                    ],
                );
            }
            for ocsp in &status.tls.ocsp {
                print_record(
                    "status_tls_ocsp",
                    [
                        ("scope", ocsp.scope.clone()),
                        ("cert_path", ocsp.cert_path.display().to_string()),
                        (
                            "staple_path",
                            ocsp.ocsp_staple_path
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "responder_urls",
                            if ocsp.responder_urls.is_empty() {
                                "-".to_string()
                            } else {
                                ocsp.responder_urls.join(",")
                            },
                        ),
                        ("nonce_mode", ocsp.nonce_mode.clone()),
                        ("responder_policy", ocsp.responder_policy.clone()),
                        ("cache_loaded", ocsp.cache_loaded.to_string()),
                        (
                            "cache_size_bytes",
                            ocsp.cache_size_bytes
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "cache_modified_unix_ms",
                            ocsp.cache_modified_unix_ms
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        ("auto_refresh_enabled", ocsp.auto_refresh_enabled.to_string()),
                        (
                            "last_refresh_unix_ms",
                            ocsp.last_refresh_unix_ms
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        ("refreshes_total", ocsp.refreshes_total.to_string()),
                        ("failures_total", ocsp.failures_total.to_string()),
                        ("last_error", ocsp.last_error.clone().unwrap_or_else(|| "-".to_string())),
                    ],
                );
            }
            for binding in &status.tls.vhost_bindings {
                print_record(
                    "status_tls_vhost_binding",
                    [
                        ("listener", binding.listener_name.clone()),
                        ("vhost", binding.vhost_id.clone()),
                        (
                            "server_names",
                            if binding.server_names.is_empty() {
                                "-".to_string()
                            } else {
                                binding.server_names.join(",")
                            },
                        ),
                        (
                            "certificate_scopes",
                            if binding.certificate_scopes.is_empty() {
                                "-".to_string()
                            } else {
                                binding.certificate_scopes.join(",")
                            },
                        ),
                        (
                            "fingerprints",
                            if binding.fingerprints.is_empty() {
                                "-".to_string()
                            } else {
                                binding.fingerprints.join(",")
                            },
                        ),
                        ("default_selected", binding.default_selected.to_string()),
                    ],
                );
            }
            for binding in &status.tls.sni_bindings {
                print_record(
                    "status_tls_sni_binding",
                    [
                        ("listener", binding.listener_name.clone()),
                        ("server_name", binding.server_name.clone()),
                        (
                            "certificate_scopes",
                            if binding.certificate_scopes.is_empty() {
                                "-".to_string()
                            } else {
                                binding.certificate_scopes.join(",")
                            },
                        ),
                        (
                            "fingerprints",
                            if binding.fingerprints.is_empty() {
                                "-".to_string()
                            } else {
                                binding.fingerprints.join(",")
                            },
                        ),
                        ("default_selected", binding.default_selected.to_string()),
                    ],
                );
            }
            for binding in &status.tls.sni_conflicts {
                print_record(
                    "status_tls_sni_conflict",
                    [
                        ("listener", binding.listener_name.clone()),
                        ("server_name", binding.server_name.clone()),
                        (
                            "certificate_scopes",
                            if binding.certificate_scopes.is_empty() {
                                "-".to_string()
                            } else {
                                binding.certificate_scopes.join(",")
                            },
                        ),
                        (
                            "fingerprints",
                            if binding.fingerprints.is_empty() {
                                "-".to_string()
                            } else {
                                binding.fingerprints.join(",")
                            },
                        ),
                    ],
                );
            }
            for binding in &status.tls.default_certificate_bindings {
                print_record(
                    "status_tls_default_certificate_binding",
                    [
                        ("listener", binding.listener_name.clone()),
                        ("server_name", binding.server_name.clone()),
                        (
                            "certificate_scopes",
                            if binding.certificate_scopes.is_empty() {
                                "-".to_string()
                            } else {
                                binding.certificate_scopes.join(",")
                            },
                        ),
                        (
                            "fingerprints",
                            if binding.fingerprints.is_empty() {
                                "-".to_string()
                            } else {
                                binding.fingerprints.join(",")
                            },
                        ),
                    ],
                );
            }
            for upstream_tls in &status.upstream_tls {
                print_record(
                    "status_upstream_tls",
                    [
                        ("upstream", upstream_tls.upstream_name.clone()),
                        ("protocol", upstream_tls.protocol.clone()),
                        ("verify_mode", upstream_tls.verify_mode.clone()),
                        (
                            "tls_versions",
                            render_optional_string_list(upstream_tls.tls_versions.as_deref()),
                        ),
                        ("server_name_enabled", upstream_tls.server_name_enabled.to_string()),
                        (
                            "server_name_override",
                            upstream_tls
                                .server_name_override
                                .clone()
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "verify_depth",
                            upstream_tls
                                .verify_depth
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        ("crl_configured", upstream_tls.crl_configured.to_string()),
                        (
                            "client_identity_configured",
                            upstream_tls.client_identity_configured.to_string(),
                        ),
                    ],
                );
            }
            Ok(())
        }
        response => Err(unexpected_admin_response("status", &response)),
    }
}
