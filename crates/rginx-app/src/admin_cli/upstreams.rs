use super::render::{print_record, render_optional_string_list};
use super::socket::{query_admin_socket, unexpected_admin_response};
use super::*;

pub(super) fn print_admin_upstreams(config_path: &Path, args: &WindowArgs) -> anyhow::Result<()> {
    match query_admin_socket(
        config_path,
        AdminRequest::GetUpstreamStats { window_secs: args.window_secs },
    )? {
        AdminResponse::UpstreamStats(upstreams) => {
            for upstream in upstreams {
                let upstream_name = upstream.upstream_name.clone();
                print_record(
                    "upstream_stats",
                    [
                        ("upstream", upstream_name.clone()),
                        (
                            "downstream_requests_total",
                            upstream.downstream_requests_total.to_string(),
                        ),
                        ("peer_attempts_total", upstream.peer_attempts_total.to_string()),
                        ("peer_successes_total", upstream.peer_successes_total.to_string()),
                        ("peer_failures_total", upstream.peer_failures_total.to_string()),
                        ("peer_timeouts_total", upstream.peer_timeouts_total.to_string()),
                        ("failovers_total", upstream.failovers_total.to_string()),
                        (
                            "completed_responses_total",
                            upstream.completed_responses_total.to_string(),
                        ),
                        (
                            "bad_gateway_responses_total",
                            upstream.bad_gateway_responses_total.to_string(),
                        ),
                        (
                            "gateway_timeout_responses_total",
                            upstream.gateway_timeout_responses_total.to_string(),
                        ),
                        (
                            "bad_request_responses_total",
                            upstream.bad_request_responses_total.to_string(),
                        ),
                        (
                            "payload_too_large_responses_total",
                            upstream.payload_too_large_responses_total.to_string(),
                        ),
                        (
                            "unsupported_media_type_responses_total",
                            upstream.unsupported_media_type_responses_total.to_string(),
                        ),
                        ("no_healthy_peers_total", upstream.no_healthy_peers_total.to_string()),
                        ("tls_protocol", upstream.tls.protocol.clone()),
                        ("tls_verify_mode", upstream.tls.verify_mode.clone()),
                        (
                            "tls_versions",
                            render_optional_string_list(upstream.tls.tls_versions.as_deref()),
                        ),
                        ("tls_server_name_enabled", upstream.tls.server_name_enabled.to_string()),
                        (
                            "tls_server_name_override",
                            upstream
                                .tls
                                .server_name_override
                                .clone()
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        (
                            "tls_verify_depth",
                            upstream
                                .tls
                                .verify_depth
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "-".to_string()),
                        ),
                        ("tls_crl_configured", upstream.tls.crl_configured.to_string()),
                        (
                            "tls_client_identity_configured",
                            upstream.tls.client_identity_configured.to_string(),
                        ),
                        (
                            "tls_failures_unknown_ca_total",
                            upstream.tls_failures_unknown_ca_total.to_string(),
                        ),
                        (
                            "tls_failures_bad_certificate_total",
                            upstream.tls_failures_bad_certificate_total.to_string(),
                        ),
                        (
                            "tls_failures_certificate_revoked_total",
                            upstream.tls_failures_certificate_revoked_total.to_string(),
                        ),
                        (
                            "tls_failures_verify_depth_exceeded_total",
                            upstream.tls_failures_verify_depth_exceeded_total.to_string(),
                        ),
                        ("recent_60s_window_secs", upstream.recent_60s.window_secs.to_string()),
                        (
                            "recent_60s_downstream_requests_total",
                            upstream.recent_60s.downstream_requests_total.to_string(),
                        ),
                        (
                            "recent_60s_peer_attempts_total",
                            upstream.recent_60s.peer_attempts_total.to_string(),
                        ),
                        (
                            "recent_60s_completed_responses_total",
                            upstream.recent_60s.completed_responses_total.to_string(),
                        ),
                        (
                            "recent_60s_bad_gateway_responses_total",
                            upstream.recent_60s.bad_gateway_responses_total.to_string(),
                        ),
                        (
                            "recent_60s_gateway_timeout_responses_total",
                            upstream.recent_60s.gateway_timeout_responses_total.to_string(),
                        ),
                        (
                            "recent_60s_failovers_total",
                            upstream.recent_60s.failovers_total.to_string(),
                        ),
                    ],
                );
                if let Some(recent_window) = &upstream.recent_window {
                    print_record(
                        "upstream_stats_recent_window",
                        [
                            ("upstream", upstream_name.clone()),
                            ("recent_window_secs", recent_window.window_secs.to_string()),
                            (
                                "recent_window_downstream_requests_total",
                                recent_window.downstream_requests_total.to_string(),
                            ),
                            (
                                "recent_window_peer_attempts_total",
                                recent_window.peer_attempts_total.to_string(),
                            ),
                            (
                                "recent_window_completed_responses_total",
                                recent_window.completed_responses_total.to_string(),
                            ),
                            (
                                "recent_window_bad_gateway_responses_total",
                                recent_window.bad_gateway_responses_total.to_string(),
                            ),
                            (
                                "recent_window_gateway_timeout_responses_total",
                                recent_window.gateway_timeout_responses_total.to_string(),
                            ),
                            (
                                "recent_window_failovers_total",
                                recent_window.failovers_total.to_string(),
                            ),
                        ],
                    );
                }
                for peer in upstream.peers {
                    print_record(
                        "upstream_stats_peer",
                        [
                            ("upstream", upstream_name.clone()),
                            ("peer", peer.peer_url),
                            ("attempts_total", peer.attempts_total.to_string()),
                            ("successes_total", peer.successes_total.to_string()),
                            ("failures_total", peer.failures_total.to_string()),
                            ("timeouts_total", peer.timeouts_total.to_string()),
                        ],
                    );
                }
            }
            Ok(())
        }
        response => Err(unexpected_admin_response("upstreams", &response)),
    }
}
