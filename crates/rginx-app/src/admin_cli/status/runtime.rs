use std::path::Path;

use crate::admin_cli::render::{
    print_record, render_last_reload, render_reload_active_revision,
    render_reload_rollback_revision, render_reload_tls_certificate_changes,
};

pub(super) fn print_status_summary(status: &rginx_http::RuntimeStatusSnapshot) {
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
                listener
                    .bindings
                    .iter()
                    .map(|binding| format!("{}://{}", binding.transport, binding.listen_addr))
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
            ("http3_active_request_streams", status.http3_active_request_streams.to_string()),
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
            ("tls_expiring_certificates", status.tls.expiring_certificate_count.to_string()),
            ("mtls_listeners", status.mtls.configured_listeners.to_string()),
            ("mtls_optional_listeners", status.mtls.optional_listeners.to_string()),
            ("mtls_required_listeners", status.mtls.required_listeners.to_string()),
            ("mtls_authenticated_connections", status.mtls.authenticated_connections.to_string()),
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
}
