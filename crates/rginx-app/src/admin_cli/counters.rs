use super::render::print_record;
use super::socket::{query_admin_socket, unexpected_admin_response};
use super::*;

pub(super) fn print_admin_counters(config_path: &Path) -> anyhow::Result<()> {
    match query_admin_socket(config_path, AdminRequest::GetCounters)? {
        AdminResponse::Counters(counters) => {
            print_record(
                "counters",
                [
                    (
                        "downstream_connections_accepted_total",
                        counters.downstream_connections_accepted.to_string(),
                    ),
                    (
                        "downstream_connections_rejected_total",
                        counters.downstream_connections_rejected.to_string(),
                    ),
                    ("downstream_requests_total", counters.downstream_requests.to_string()),
                    ("downstream_responses_total", counters.downstream_responses.to_string()),
                    (
                        "downstream_responses_1xx_total",
                        counters.downstream_responses_1xx.to_string(),
                    ),
                    (
                        "downstream_responses_2xx_total",
                        counters.downstream_responses_2xx.to_string(),
                    ),
                    (
                        "downstream_responses_3xx_total",
                        counters.downstream_responses_3xx.to_string(),
                    ),
                    (
                        "downstream_responses_4xx_total",
                        counters.downstream_responses_4xx.to_string(),
                    ),
                    (
                        "downstream_responses_5xx_total",
                        counters.downstream_responses_5xx.to_string(),
                    ),
                    (
                        "downstream_mtls_authenticated_connections_total",
                        counters.downstream_mtls_authenticated_connections.to_string(),
                    ),
                    (
                        "downstream_mtls_authenticated_requests_total",
                        counters.downstream_mtls_authenticated_requests.to_string(),
                    ),
                    (
                        "downstream_mtls_anonymous_requests_total",
                        counters.downstream_mtls_anonymous_requests.to_string(),
                    ),
                    (
                        "downstream_tls_handshake_failures_total",
                        counters.downstream_tls_handshake_failures.to_string(),
                    ),
                    (
                        "downstream_tls_handshake_failures_missing_client_cert_total",
                        counters.downstream_tls_handshake_failures_missing_client_cert.to_string(),
                    ),
                    (
                        "downstream_tls_handshake_failures_unknown_ca_total",
                        counters.downstream_tls_handshake_failures_unknown_ca.to_string(),
                    ),
                    (
                        "downstream_tls_handshake_failures_bad_certificate_total",
                        counters.downstream_tls_handshake_failures_bad_certificate.to_string(),
                    ),
                    (
                        "downstream_tls_handshake_failures_certificate_revoked_total",
                        counters.downstream_tls_handshake_failures_certificate_revoked.to_string(),
                    ),
                    (
                        "downstream_tls_handshake_failures_verify_depth_exceeded_total",
                        counters
                            .downstream_tls_handshake_failures_verify_depth_exceeded
                            .to_string(),
                    ),
                    (
                        "downstream_tls_handshake_failures_other_total",
                        counters.downstream_tls_handshake_failures_other.to_string(),
                    ),
                ],
            );
            Ok(())
        }
        response => Err(unexpected_admin_response("counters", &response)),
    }
}
