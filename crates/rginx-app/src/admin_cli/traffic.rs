use super::render::print_record;
use super::socket::{query_admin_socket, unexpected_admin_response};
use super::*;

pub(super) fn print_admin_traffic(config_path: &Path, args: &WindowArgs) -> anyhow::Result<()> {
    match query_admin_socket(
        config_path,
        AdminRequest::GetTrafficStats { window_secs: args.window_secs },
    )? {
        AdminResponse::TrafficStats(traffic) => {
            for listener in traffic.listeners {
                let listener_id = listener.listener_id.clone();
                print_record(
                    "traffic_listener",
                    [
                        ("listener", listener_id.clone()),
                        ("name", listener.listener_name),
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
                        (
                            "downstream_responses_1xx_total",
                            listener.downstream_responses_1xx.to_string(),
                        ),
                        (
                            "downstream_responses_2xx_total",
                            listener.downstream_responses_2xx.to_string(),
                        ),
                        (
                            "downstream_responses_3xx_total",
                            listener.downstream_responses_3xx.to_string(),
                        ),
                        (
                            "downstream_responses_4xx_total",
                            listener.downstream_responses_4xx.to_string(),
                        ),
                        (
                            "downstream_responses_5xx_total",
                            listener.downstream_responses_5xx.to_string(),
                        ),
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
                        (
                            "grpc_protocol_grpc_web_total",
                            listener.grpc.protocol_grpc_web_total.to_string(),
                        ),
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
                            ("listener", listener_id.clone()),
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
            for vhost in traffic.vhosts {
                let vhost_id = vhost.vhost_id.clone();
                print_record(
                    "traffic_vhost",
                    [
                        ("vhost", vhost_id.clone()),
                        (
                            "server_names",
                            if vhost.server_names.is_empty() {
                                "-".to_string()
                            } else {
                                vhost.server_names.join(",")
                            },
                        ),
                        ("downstream_requests_total", vhost.downstream_requests.to_string()),
                        ("unmatched_requests_total", vhost.unmatched_requests_total.to_string()),
                        ("downstream_responses_total", vhost.downstream_responses.to_string()),
                        (
                            "downstream_responses_1xx_total",
                            vhost.downstream_responses_1xx.to_string(),
                        ),
                        (
                            "downstream_responses_2xx_total",
                            vhost.downstream_responses_2xx.to_string(),
                        ),
                        (
                            "downstream_responses_3xx_total",
                            vhost.downstream_responses_3xx.to_string(),
                        ),
                        (
                            "downstream_responses_4xx_total",
                            vhost.downstream_responses_4xx.to_string(),
                        ),
                        (
                            "downstream_responses_5xx_total",
                            vhost.downstream_responses_5xx.to_string(),
                        ),
                        ("recent_60s_window_secs", vhost.recent_60s.window_secs.to_string()),
                        (
                            "recent_60s_downstream_requests_total",
                            vhost.recent_60s.downstream_requests_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_total",
                            vhost.recent_60s.downstream_responses_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_2xx_total",
                            vhost.recent_60s.downstream_responses_2xx_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_4xx_total",
                            vhost.recent_60s.downstream_responses_4xx_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_5xx_total",
                            vhost.recent_60s.downstream_responses_5xx_total.to_string(),
                        ),
                        (
                            "recent_60s_grpc_requests_total",
                            vhost.recent_60s.grpc_requests_total.to_string(),
                        ),
                        ("grpc_requests_total", vhost.grpc.requests_total.to_string()),
                        ("grpc_protocol_grpc_total", vhost.grpc.protocol_grpc_total.to_string()),
                        (
                            "grpc_protocol_grpc_web_total",
                            vhost.grpc.protocol_grpc_web_total.to_string(),
                        ),
                        (
                            "grpc_protocol_grpc_web_text_total",
                            vhost.grpc.protocol_grpc_web_text_total.to_string(),
                        ),
                        ("grpc_status_0_total", vhost.grpc.status_0_total.to_string()),
                        ("grpc_status_1_total", vhost.grpc.status_1_total.to_string()),
                        ("grpc_status_3_total", vhost.grpc.status_3_total.to_string()),
                        ("grpc_status_4_total", vhost.grpc.status_4_total.to_string()),
                        ("grpc_status_7_total", vhost.grpc.status_7_total.to_string()),
                        ("grpc_status_8_total", vhost.grpc.status_8_total.to_string()),
                        ("grpc_status_12_total", vhost.grpc.status_12_total.to_string()),
                        ("grpc_status_14_total", vhost.grpc.status_14_total.to_string()),
                        ("grpc_status_other_total", vhost.grpc.status_other_total.to_string()),
                    ],
                );
                if let Some(recent_window) = &vhost.recent_window {
                    print_record(
                        "traffic_vhost_recent_window",
                        [
                            ("vhost", vhost_id),
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
            }
            for route in traffic.routes {
                let route_id = route.route_id.clone();
                print_record(
                    "traffic_route",
                    [
                        ("route", route_id.clone()),
                        ("vhost", route.vhost_id.clone()),
                        ("downstream_requests_total", route.downstream_requests.to_string()),
                        ("downstream_responses_total", route.downstream_responses.to_string()),
                        (
                            "downstream_responses_1xx_total",
                            route.downstream_responses_1xx.to_string(),
                        ),
                        (
                            "downstream_responses_2xx_total",
                            route.downstream_responses_2xx.to_string(),
                        ),
                        (
                            "downstream_responses_3xx_total",
                            route.downstream_responses_3xx.to_string(),
                        ),
                        (
                            "downstream_responses_4xx_total",
                            route.downstream_responses_4xx.to_string(),
                        ),
                        (
                            "downstream_responses_5xx_total",
                            route.downstream_responses_5xx.to_string(),
                        ),
                        ("access_denied_total", route.access_denied_total.to_string()),
                        ("rate_limited_total", route.rate_limited_total.to_string()),
                        ("recent_60s_window_secs", route.recent_60s.window_secs.to_string()),
                        (
                            "recent_60s_downstream_requests_total",
                            route.recent_60s.downstream_requests_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_total",
                            route.recent_60s.downstream_responses_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_2xx_total",
                            route.recent_60s.downstream_responses_2xx_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_4xx_total",
                            route.recent_60s.downstream_responses_4xx_total.to_string(),
                        ),
                        (
                            "recent_60s_downstream_responses_5xx_total",
                            route.recent_60s.downstream_responses_5xx_total.to_string(),
                        ),
                        (
                            "recent_60s_grpc_requests_total",
                            route.recent_60s.grpc_requests_total.to_string(),
                        ),
                        ("grpc_requests_total", route.grpc.requests_total.to_string()),
                        ("grpc_protocol_grpc_total", route.grpc.protocol_grpc_total.to_string()),
                        (
                            "grpc_protocol_grpc_web_total",
                            route.grpc.protocol_grpc_web_total.to_string(),
                        ),
                        (
                            "grpc_protocol_grpc_web_text_total",
                            route.grpc.protocol_grpc_web_text_total.to_string(),
                        ),
                        ("grpc_status_0_total", route.grpc.status_0_total.to_string()),
                        ("grpc_status_1_total", route.grpc.status_1_total.to_string()),
                        ("grpc_status_3_total", route.grpc.status_3_total.to_string()),
                        ("grpc_status_4_total", route.grpc.status_4_total.to_string()),
                        ("grpc_status_7_total", route.grpc.status_7_total.to_string()),
                        ("grpc_status_8_total", route.grpc.status_8_total.to_string()),
                        ("grpc_status_12_total", route.grpc.status_12_total.to_string()),
                        ("grpc_status_14_total", route.grpc.status_14_total.to_string()),
                        ("grpc_status_other_total", route.grpc.status_other_total.to_string()),
                    ],
                );
                if let Some(recent_window) = &route.recent_window {
                    print_record(
                        "traffic_route_recent_window",
                        [
                            ("route", route_id),
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
            }
            Ok(())
        }
        response => Err(unexpected_admin_response("traffic", &response)),
    }
}
