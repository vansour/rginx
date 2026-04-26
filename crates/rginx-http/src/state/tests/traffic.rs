use super::*;

#[test]
fn traffic_stats_snapshot_tracks_listener_vhost_and_route_counters() {
    let shared = SharedState::from_config(snapshot_with_routes("127.0.0.1:8080"))
        .expect("shared state should build");

    shared.record_connection_accepted("default");
    shared.record_connection_rejected("default");
    shared.record_downstream_request("default", "server", Some("server/routes[0]|exact:/"));
    shared.record_route_access_denied("server/routes[0]|exact:/");
    shared.record_downstream_response(
        "default",
        "server",
        Some("server/routes[0]|exact:/"),
        StatusCode::FORBIDDEN,
    );
    shared.record_downstream_request("default", "server", Some("server/routes[0]|exact:/"));
    shared.record_route_rate_limited("server/routes[0]|exact:/");
    shared.record_downstream_response(
        "default",
        "server",
        Some("server/routes[0]|exact:/"),
        StatusCode::TOO_MANY_REQUESTS,
    );
    shared.record_downstream_request("default", "server", Some("server/routes[0]|exact:/"));
    shared.record_downstream_response(
        "default",
        "server",
        Some("server/routes[0]|exact:/"),
        StatusCode::OK,
    );

    let snapshot = shared.traffic_stats_snapshot();
    assert_eq!(snapshot.listeners.len(), 1);
    assert_eq!(snapshot.listeners[0].listener_id, "default");
    assert_eq!(snapshot.listeners[0].downstream_connections_accepted, 1);
    assert_eq!(snapshot.listeners[0].downstream_connections_rejected, 1);
    assert_eq!(snapshot.listeners[0].downstream_requests, 3);
    assert_eq!(snapshot.listeners[0].unmatched_requests_total, 0);
    assert_eq!(snapshot.listeners[0].downstream_responses, 3);
    assert_eq!(snapshot.listeners[0].downstream_responses_2xx, 1);
    assert_eq!(snapshot.listeners[0].downstream_responses_4xx, 2);

    assert_eq!(snapshot.vhosts.len(), 1);
    assert_eq!(snapshot.vhosts[0].vhost_id, "server");
    assert_eq!(snapshot.vhosts[0].downstream_requests, 3);
    assert_eq!(snapshot.vhosts[0].unmatched_requests_total, 0);
    assert_eq!(snapshot.vhosts[0].downstream_responses, 3);
    assert_eq!(snapshot.vhosts[0].downstream_responses_2xx, 1);
    assert_eq!(snapshot.vhosts[0].downstream_responses_4xx, 2);

    assert_eq!(snapshot.routes.len(), 1);
    assert_eq!(snapshot.routes[0].route_id, "server/routes[0]|exact:/");
    assert_eq!(snapshot.routes[0].vhost_id, "server");
    assert_eq!(snapshot.routes[0].downstream_requests, 3);
    assert_eq!(snapshot.routes[0].downstream_responses, 3);
    assert_eq!(snapshot.routes[0].downstream_responses_2xx, 1);
    assert_eq!(snapshot.routes[0].downstream_responses_4xx, 2);
    assert_eq!(snapshot.routes[0].access_denied_total, 1);
    assert_eq!(snapshot.routes[0].rate_limited_total, 1);
    assert_eq!(snapshot.listeners[0].recent_60s.window_secs, 60);
    assert_eq!(snapshot.listeners[0].recent_60s.downstream_requests_total, 3);
    assert_eq!(snapshot.listeners[0].recent_60s.downstream_responses_total, 3);
    assert_eq!(snapshot.listeners[0].recent_60s.downstream_responses_2xx_total, 1);
    assert_eq!(snapshot.listeners[0].recent_60s.downstream_responses_4xx_total, 2);
    assert_eq!(snapshot.listeners[0].recent_60s.downstream_responses_5xx_total, 0);
}

#[test]
fn traffic_stats_snapshot_tracks_unmatched_requests_per_listener_and_vhost() {
    let shared = SharedState::from_config(snapshot_with_routes("127.0.0.1:8080"))
        .expect("shared state should build");

    shared.record_downstream_request("default", "server", None);
    shared.record_downstream_response("default", "server", None, StatusCode::NOT_FOUND);

    let snapshot = shared.traffic_stats_snapshot();
    assert_eq!(snapshot.listeners.len(), 1);
    assert_eq!(snapshot.listeners[0].downstream_requests, 1);
    assert_eq!(snapshot.listeners[0].unmatched_requests_total, 1);
    assert_eq!(snapshot.listeners[0].downstream_responses_4xx, 1);

    assert_eq!(snapshot.vhosts.len(), 1);
    assert_eq!(snapshot.vhosts[0].downstream_requests, 1);
    assert_eq!(snapshot.vhosts[0].unmatched_requests_total, 1);
    assert_eq!(snapshot.vhosts[0].downstream_responses_4xx, 1);

    assert_eq!(snapshot.routes.len(), 1);
    assert_eq!(snapshot.routes[0].downstream_requests, 0);
    assert_eq!(snapshot.routes[0].downstream_responses, 0);
}

#[test]
fn traffic_stats_snapshot_tracks_grpc_protocols_and_statuses() {
    let shared = SharedState::from_config(snapshot_with_routes("127.0.0.1:8080"))
        .expect("shared state should build");

    shared.record_grpc_request("default", "server", Some("server/routes[0]|exact:/"), "grpc");
    shared.record_grpc_status("default", "server", Some("server/routes[0]|exact:/"), Some("0"));
    shared.record_grpc_request("default", "server", Some("server/routes[0]|exact:/"), "grpc-web");
    shared.record_grpc_status("default", "server", Some("server/routes[0]|exact:/"), Some("14"));
    shared.record_grpc_request(
        "default",
        "server",
        Some("server/routes[0]|exact:/"),
        "grpc-web-text",
    );
    shared.record_grpc_status(
        "default",
        "server",
        Some("server/routes[0]|exact:/"),
        Some("custom"),
    );

    let snapshot = shared.traffic_stats_snapshot();
    assert_eq!(snapshot.listeners[0].grpc.requests_total, 3);
    assert_eq!(snapshot.listeners[0].grpc.protocol_grpc_total, 1);
    assert_eq!(snapshot.listeners[0].grpc.protocol_grpc_web_total, 1);
    assert_eq!(snapshot.listeners[0].grpc.protocol_grpc_web_text_total, 1);
    assert_eq!(snapshot.listeners[0].grpc.status_0_total, 1);
    assert_eq!(snapshot.listeners[0].grpc.status_14_total, 1);
    assert_eq!(snapshot.listeners[0].grpc.status_other_total, 1);

    assert_eq!(snapshot.vhosts[0].grpc.requests_total, 3);
    assert_eq!(snapshot.vhosts[0].grpc.status_0_total, 1);
    assert_eq!(snapshot.vhosts[0].grpc.status_14_total, 1);
    assert_eq!(snapshot.vhosts[0].grpc.status_other_total, 1);

    assert_eq!(snapshot.routes[0].grpc.requests_total, 3);
    assert_eq!(snapshot.routes[0].grpc.status_0_total, 1);
    assert_eq!(snapshot.routes[0].grpc.status_14_total, 1);
    assert_eq!(snapshot.routes[0].grpc.status_other_total, 1);
}
