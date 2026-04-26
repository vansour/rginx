use crate::admin_cli::render::print_record;

pub(super) fn print_traffic_listeners(listeners: &[rginx_http::ListenerStatsSnapshot]) {
    for listener in listeners {
        let listener_id = listener.listener_id.clone();
        print_record(
            "traffic_listener",
            [
                ("listener", listener_id.clone()),
                ("name", listener.listener_name.clone()),
                ("listen", listener.listen_addr.to_string()),
                ("active_connections", listener.active_connections.to_string()),
                (
                    "downstream_connections_accepted_total",
                    listener.downstream_connections_accepted.to_string(),
                ),
                (
                    "downstream_connections_rejected_total",
                    listener.downstream_connections_rejected.to_string(),
                ),
                ("downstream_requests_total", listener.downstream_requests.to_string()),
                ("unmatched_requests_total", listener.unmatched_requests_total.to_string()),
                ("downstream_responses_total", listener.downstream_responses.to_string()),
                ("downstream_responses_1xx_total", listener.downstream_responses_1xx.to_string()),
                ("downstream_responses_2xx_total", listener.downstream_responses_2xx.to_string()),
                ("downstream_responses_3xx_total", listener.downstream_responses_3xx.to_string()),
                ("downstream_responses_4xx_total", listener.downstream_responses_4xx.to_string()),
                ("downstream_responses_5xx_total", listener.downstream_responses_5xx.to_string()),
                ("recent_60s_window_secs", listener.recent_60s.window_secs.to_string()),
                (
                    "recent_60s_downstream_requests_total",
                    listener.recent_60s.downstream_requests_total.to_string(),
                ),
                (
                    "recent_60s_downstream_responses_total",
                    listener.recent_60s.downstream_responses_total.to_string(),
                ),
                (
                    "recent_60s_downstream_responses_2xx_total",
                    listener.recent_60s.downstream_responses_2xx_total.to_string(),
                ),
                (
                    "recent_60s_downstream_responses_4xx_total",
                    listener.recent_60s.downstream_responses_4xx_total.to_string(),
                ),
                (
                    "recent_60s_downstream_responses_5xx_total",
                    listener.recent_60s.downstream_responses_5xx_total.to_string(),
                ),
                (
                    "recent_60s_grpc_requests_total",
                    listener.recent_60s.grpc_requests_total.to_string(),
                ),
                ("grpc_requests_total", listener.grpc.requests_total.to_string()),
                ("grpc_protocol_grpc_total", listener.grpc.protocol_grpc_total.to_string()),
                ("grpc_protocol_grpc_web_total", listener.grpc.protocol_grpc_web_total.to_string()),
                (
                    "grpc_protocol_grpc_web_text_total",
                    listener.grpc.protocol_grpc_web_text_total.to_string(),
                ),
                ("grpc_status_0_total", listener.grpc.status_0_total.to_string()),
                ("grpc_status_1_total", listener.grpc.status_1_total.to_string()),
                ("grpc_status_3_total", listener.grpc.status_3_total.to_string()),
                ("grpc_status_4_total", listener.grpc.status_4_total.to_string()),
                ("grpc_status_7_total", listener.grpc.status_7_total.to_string()),
                ("grpc_status_8_total", listener.grpc.status_8_total.to_string()),
                ("grpc_status_12_total", listener.grpc.status_12_total.to_string()),
                ("grpc_status_14_total", listener.grpc.status_14_total.to_string()),
                ("grpc_status_other_total", listener.grpc.status_other_total.to_string()),
            ],
        );

        if let Some(recent_window) = &listener.recent_window {
            print_record(
                "traffic_listener_recent_window",
                [
                    ("listener", listener_id.clone()),
                    ("recent_window_secs", recent_window.window_secs.to_string()),
                    (
                        "recent_window_downstream_requests_total",
                        recent_window.downstream_requests_total.to_string(),
                    ),
                    (
                        "recent_window_downstream_responses_total",
                        recent_window.downstream_responses_total.to_string(),
                    ),
                    (
                        "recent_window_downstream_responses_2xx_total",
                        recent_window.downstream_responses_2xx_total.to_string(),
                    ),
                    (
                        "recent_window_downstream_responses_4xx_total",
                        recent_window.downstream_responses_4xx_total.to_string(),
                    ),
                    (
                        "recent_window_downstream_responses_5xx_total",
                        recent_window.downstream_responses_5xx_total.to_string(),
                    ),
                    (
                        "recent_window_grpc_requests_total",
                        recent_window.grpc_requests_total.to_string(),
                    ),
                ],
            );
        }

        if let Some(http3) = &listener.http3_runtime {
            print_record(
                "traffic_listener_http3",
                [
                    ("listener", listener_id),
                    ("active_connections", http3.active_connections.to_string()),
                    ("active_request_streams", http3.active_request_streams.to_string()),
                    ("retry_issued_total", http3.retry_issued_total.to_string()),
                    ("retry_failed_total", http3.retry_failed_total.to_string()),
                    ("request_accept_errors_total", http3.request_accept_errors_total.to_string()),
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
}
