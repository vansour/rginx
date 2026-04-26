use crate::admin_cli::render::print_record;
use rginx_http::state::RecentTrafficStatsSnapshot;

pub(super) fn print_recent_window_record(
    kind: &'static str,
    parent_key: &'static str,
    parent_id: &str,
    recent: &RecentTrafficStatsSnapshot,
) {
    print_record(
        kind,
        [
            (parent_key, parent_id.to_string()),
            ("recent_window_secs", recent.window_secs.to_string()),
            (
                "recent_window_downstream_requests_total",
                recent.downstream_requests_total.to_string(),
            ),
            (
                "recent_window_downstream_responses_total",
                recent.downstream_responses_total.to_string(),
            ),
            (
                "recent_window_downstream_responses_2xx_total",
                recent.downstream_responses_2xx_total.to_string(),
            ),
            (
                "recent_window_downstream_responses_4xx_total",
                recent.downstream_responses_4xx_total.to_string(),
            ),
            (
                "recent_window_downstream_responses_5xx_total",
                recent.downstream_responses_5xx_total.to_string(),
            ),
            ("recent_window_grpc_requests_total", recent.grpc_requests_total.to_string()),
        ],
    );
}
