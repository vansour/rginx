use crate::admin_cli::render::print_record;

use super::render::print_recent_window_record;

pub(super) fn print_traffic_routes(routes: &[rginx_http::RouteStatsSnapshot]) {
    for route in routes {
        let route_id = route.route_id.clone();
        print_record(
            "traffic_route",
            [
                ("route", route_id.clone()),
                ("vhost", route.vhost_id.clone()),
                ("downstream_requests_total", route.downstream_requests.to_string()),
                ("downstream_responses_total", route.downstream_responses.to_string()),
                ("downstream_responses_1xx_total", route.downstream_responses_1xx.to_string()),
                ("downstream_responses_2xx_total", route.downstream_responses_2xx.to_string()),
                ("downstream_responses_3xx_total", route.downstream_responses_3xx.to_string()),
                ("downstream_responses_4xx_total", route.downstream_responses_4xx.to_string()),
                ("downstream_responses_5xx_total", route.downstream_responses_5xx.to_string()),
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
                ("grpc_protocol_grpc_web_total", route.grpc.protocol_grpc_web_total.to_string()),
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
            print_recent_window_record(
                "traffic_route_recent_window",
                "route",
                &route_id,
                recent_window,
            );
        }
    }
}
