use crate::admin_cli::render::print_record;

pub(super) fn print_traffic_vhosts(vhosts: &[rginx_http::VhostStatsSnapshot]) {
    for vhost in vhosts {
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
                ("downstream_responses_1xx_total", vhost.downstream_responses_1xx.to_string()),
                ("downstream_responses_2xx_total", vhost.downstream_responses_2xx.to_string()),
                ("downstream_responses_3xx_total", vhost.downstream_responses_3xx.to_string()),
                ("downstream_responses_4xx_total", vhost.downstream_responses_4xx.to_string()),
                ("downstream_responses_5xx_total", vhost.downstream_responses_5xx.to_string()),
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
                ("grpc_protocol_grpc_web_total", vhost.grpc.protocol_grpc_web_total.to_string()),
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
}
