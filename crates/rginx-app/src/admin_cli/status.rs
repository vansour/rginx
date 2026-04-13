use super::render::{
    print_record, render_last_reload, render_optional_string_list, render_reload_active_revision,
    render_reload_rollback_revision, render_reload_tls_certificate_changes,
};
use super::socket::{query_admin_socket, unexpected_admin_response};
use super::*;

pub(super) fn print_admin_status(config_path: &Path) -> anyhow::Result<()> {
    match query_admin_socket(config_path, AdminRequest::GetStatus)? {
        AdminResponse::Status(status) => {
            let listen_addrs = if status.listeners.is_empty() {
                "-".to_string()
            } else {
                status
                    .listeners
                    .iter()
                    .map(|listener| listener.listen_addr.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let bind_addrs = {
                let bind_addrs = status
                    .listeners
                    .iter()
                    .flat_map(|listener| {
                        listener.bindings.iter().map(|binding| {
                            format!("{}://{}", binding.transport, binding.listen_addr)
                        })
                    })
                    .collect::<Vec<_>>();
                if bind_addrs.is_empty() { "-".to_string() } else { bind_addrs.join(",") }
            };
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
                    ("listeners", status.listeners.len().to_string()),
                    (
                        "listener_bindings",
                        status
                            .listeners
                            .iter()
                            .map(|listener| listener.binding_count)
                            .sum::<usize>()
                            .to_string(),
                    ),
                    ("listen_addrs", listen_addrs),
                    ("bind_addrs", bind_addrs),
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
                    (
                        "http3",
                        if status.listeners.iter().any(|listener| listener.http3_enabled) {
                            "enabled"
                        } else {
                            "disabled"
                        }
                        .to_string(),
                    ),
                    (
                        "http3_early_data_enabled_listeners",
                        status.http3_early_data_enabled_listeners.to_string(),
                    ),
                    ("http3_active_connections", status.http3_active_connections.to_string()),
                    (
                        "http3_active_request_streams",
                        status.http3_active_request_streams.to_string(),
                    ),
                    ("http3_retry_issued_total", status.http3_retry_issued_total.to_string()),
                    ("http3_retry_failed_total", status.http3_retry_failed_total.to_string()),
                    (
                        "http3_request_accept_errors_total",
                        status.http3_request_accept_errors_total.to_string(),
                    ),
                    (
                        "http3_request_resolve_errors_total",
                        status.http3_request_resolve_errors_total.to_string(),
                    ),
                    (
                        "http3_request_body_stream_errors_total",
                        status.http3_request_body_stream_errors_total.to_string(),
                    ),
                    (
                        "http3_response_stream_errors_total",
                        status.http3_response_stream_errors_total.to_string(),
                    ),
                    (
                        "http3_early_data_accepted_requests",
                        status.http3_early_data_accepted_requests.to_string(),
                    ),
                    (
                        "http3_early_data_rejected_requests",
                        status.http3_early_data_rejected_requests.to_string(),
                    ),
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
            for listener in &status.listeners {
                print_record(
                    "status_listener",
                    [
                        ("listener", listener.listener_name.clone()),
                        ("listener_id", listener.listener_id.clone()),
                        ("listen", listener.listen_addr.to_string()),
                        ("transport_bindings", listener.binding_count.to_string()),
                        (
                            "tls",
                            if listener.tls_enabled { "enabled" } else { "disabled" }.to_string(),
                        ),
                        (
                            "http3",
                            if listener.http3_enabled { "enabled" } else { "disabled" }.to_string(),
                        ),
                        ("proxy_protocol", listener.proxy_protocol_enabled.to_string()),
                        (
                            "default_certificate",
                            listener.default_certificate.clone().unwrap_or_else(|| "-".to_string()),
                        ),
                        ("keep_alive", listener.keep_alive.to_string()),
                        (
                            "max_connections",
                            listener
                                .max_connections
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "access_log_format_configured",
                            listener.access_log_format_configured.to_string(),
                        ),
                    ],
                );
                for binding in &listener.bindings {
                    print_record(
                        "status_listener_binding",
                        [
                            ("listener", listener.listener_id.clone()),
                            ("binding", binding.binding_name.clone()),
                            ("transport", binding.transport.clone()),
                            ("listen", binding.listen_addr.to_string()),
                            (
                                "protocols",
                                if binding.protocols.is_empty() {
                                    "-".to_string()
                                } else {
                                    binding.protocols.join(",")
                                },
                            ),
                            ("worker_count", binding.worker_count.to_string()),
                            (
                                "reuse_port_enabled",
                                binding
                                    .reuse_port_enabled
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "-".to_string()),
                            ),
                            (
                                "advertise_alt_svc",
                                binding
                                    .advertise_alt_svc
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "-".to_string()),
                            ),
                            (
                                "alt_svc_max_age_secs",
                                binding
                                    .alt_svc_max_age_secs
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "-".to_string()),
                            ),
                            (
                                "http3_max_concurrent_streams",
                                binding
                                    .http3_max_concurrent_streams
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "-".to_string()),
                            ),
                            (
                                "http3_stream_buffer_size",
                                binding
                                    .http3_stream_buffer_size
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "-".to_string()),
                            ),
                            (
                                "http3_active_connection_id_limit",
                                binding
                                    .http3_active_connection_id_limit
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "-".to_string()),
                            ),
                            (
                                "http3_retry",
                                binding
                                    .http3_retry
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "-".to_string()),
                            ),
                            (
                                "http3_host_key_path",
                                binding
                                    .http3_host_key_path
                                    .as_ref()
                                    .map(|path| path.display().to_string())
                                    .unwrap_or_else(|| "-".to_string()),
                            ),
                            (
                                "http3_gso",
                                binding
                                    .http3_gso
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "-".to_string()),
                            ),
                            (
                                "http3_early_data_enabled",
                                binding
                                    .http3_early_data_enabled
                                    .map(|value| value.to_string())
                                    .unwrap_or_else(|| "-".to_string()),
                            ),
                        ],
                    );
                }
                if let Some(http3) = &listener.http3_runtime {
                    print_record(
                        "status_listener_http3",
                        [
                            ("listener", listener.listener_name.clone()),
                            ("listener_id", listener.listener_id.clone()),
                            ("active_connections", http3.active_connections.to_string()),
                            ("active_request_streams", http3.active_request_streams.to_string()),
                            ("retry_issued_total", http3.retry_issued_total.to_string()),
                            ("retry_failed_total", http3.retry_failed_total.to_string()),
                            (
                                "request_accept_errors_total",
                                http3.request_accept_errors_total.to_string(),
                            ),
                            (
                                "request_resolve_errors_total",
                                http3.request_resolve_errors_total.to_string(),
                            ),
                            (
                                "request_body_stream_errors_total",
                                http3.request_body_stream_errors_total.to_string(),
                            ),
                            (
                                "response_stream_errors_total",
                                http3.response_stream_errors_total.to_string(),
                            ),
                            (
                                "connection_close_version_mismatch_total",
                                http3.connection_close_version_mismatch_total.to_string(),
                            ),
                            (
                                "connection_close_transport_error_total",
                                http3.connection_close_transport_error_total.to_string(),
                            ),
                            (
                                "connection_close_connection_closed_total",
                                http3.connection_close_connection_closed_total.to_string(),
                            ),
                            (
                                "connection_close_application_closed_total",
                                http3.connection_close_application_closed_total.to_string(),
                            ),
                            (
                                "connection_close_reset_total",
                                http3.connection_close_reset_total.to_string(),
                            ),
                            (
                                "connection_close_timed_out_total",
                                http3.connection_close_timed_out_total.to_string(),
                            ),
                            (
                                "connection_close_locally_closed_total",
                                http3.connection_close_locally_closed_total.to_string(),
                            ),
                            (
                                "connection_close_cids_exhausted_total",
                                http3.connection_close_cids_exhausted_total.to_string(),
                            ),
                        ],
                    );
                }
            }
            for listener in &status.tls.listeners {
                print_record(
                    "status_tls_listener",
                    [
                        ("listener", listener.listener_name.clone()),
                        ("listener_id", listener.listener_id.clone()),
                        ("listen", listener.listen_addr.to_string()),
                        ("tls", listener.tls_enabled.to_string()),
                        ("http3_enabled", listener.http3_enabled.to_string()),
                        (
                            "http3_listen",
                            listener
                                .http3_listen_addr
                                .map(|listen_addr| listen_addr.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "default_certificate",
                            listener.default_certificate.clone().unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "tcp_versions",
                            listener
                                .versions
                                .as_ref()
                                .map(|versions| render_optional_string_list(Some(versions)))
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "tcp_alpn_protocols",
                            render_optional_string_list(Some(&listener.alpn_protocols)),
                        ),
                        (
                            "http3_versions",
                            render_optional_string_list(Some(&listener.http3_versions)),
                        ),
                        (
                            "http3_alpn_protocols",
                            render_optional_string_list(Some(&listener.http3_alpn_protocols)),
                        ),
                        (
                            "http3_max_concurrent_streams",
                            listener
                                .http3_max_concurrent_streams
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "http3_stream_buffer_size",
                            listener
                                .http3_stream_buffer_size
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "http3_active_connection_id_limit",
                            listener
                                .http3_active_connection_id_limit
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "http3_retry",
                            listener
                                .http3_retry
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "http3_host_key_path",
                            listener
                                .http3_host_key_path
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "http3_gso",
                            listener
                                .http3_gso
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "http3_early_data_enabled",
                            listener
                                .http3_early_data_enabled
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        ("sni_names", render_optional_string_list(Some(&listener.sni_names))),
                        (
                            "client_auth_mode",
                            listener.client_auth_mode.clone().unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "client_auth_verify_depth",
                            listener
                                .client_auth_verify_depth
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "client_auth_crl_configured",
                            listener.client_auth_crl_configured.to_string(),
                        ),
                    ],
                );
            }
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
