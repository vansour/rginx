use super::*;

#[test]
fn upstream_stats_snapshot_tracks_requests_attempts_and_failovers() {
    let shared = SharedState::from_config(snapshot_with_upstream("127.0.0.1:8080"))
        .expect("shared state should build");

    shared.record_upstream_request("backend");
    shared.record_upstream_peer_attempt("backend", "http://127.0.0.1:9000");
    shared.record_upstream_peer_success("backend", "http://127.0.0.1:9000");
    shared.record_upstream_request("backend");
    shared.record_upstream_peer_attempt("backend", "http://127.0.0.1:9000");
    shared.record_upstream_peer_failure("backend", "http://127.0.0.1:9000");
    shared.record_upstream_failover("backend");
    shared.record_upstream_request("backend");
    shared.record_upstream_peer_attempt("backend", "http://127.0.0.1:9000");
    shared.record_upstream_peer_timeout("backend", "http://127.0.0.1:9000");

    let snapshot = shared.upstream_stats_snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].upstream_name, "backend");
    assert_eq!(snapshot[0].downstream_requests_total, 3);
    assert_eq!(snapshot[0].peer_attempts_total, 3);
    assert_eq!(snapshot[0].peer_successes_total, 1);
    assert_eq!(snapshot[0].peer_failures_total, 1);
    assert_eq!(snapshot[0].peer_timeouts_total, 1);
    assert_eq!(snapshot[0].failovers_total, 1);
    assert_eq!(snapshot[0].completed_responses_total, 0);
    assert_eq!(snapshot[0].bad_gateway_responses_total, 0);
    assert_eq!(snapshot[0].gateway_timeout_responses_total, 0);
    assert_eq!(snapshot[0].bad_request_responses_total, 0);
    assert_eq!(snapshot[0].payload_too_large_responses_total, 0);
    assert_eq!(snapshot[0].unsupported_media_type_responses_total, 0);
    assert_eq!(snapshot[0].no_healthy_peers_total, 0);
    assert_eq!(snapshot[0].peers.len(), 1);
    assert_eq!(snapshot[0].peers[0].peer_url, "http://127.0.0.1:9000");
    assert_eq!(snapshot[0].peers[0].attempts_total, 3);
    assert_eq!(snapshot[0].peers[0].successes_total, 1);
    assert_eq!(snapshot[0].peers[0].failures_total, 1);
    assert_eq!(snapshot[0].peers[0].timeouts_total, 1);
}

#[test]
fn upstream_stats_snapshot_tracks_terminal_response_reasons() {
    let shared = SharedState::from_config(snapshot_with_upstream("127.0.0.1:8080"))
        .expect("shared state should build");

    shared.record_upstream_request("backend");
    shared.record_upstream_completed_response("backend");
    shared.record_upstream_request("backend");
    shared.record_upstream_bad_gateway_response("backend");
    shared.record_upstream_request("backend");
    shared.record_upstream_gateway_timeout_response("backend");
    shared.record_upstream_request("backend");
    shared.record_upstream_bad_request_response("backend");
    shared.record_upstream_request("backend");
    shared.record_upstream_payload_too_large_response("backend");
    shared.record_upstream_request("backend");
    shared.record_upstream_unsupported_media_type_response("backend");
    shared.record_upstream_request("backend");
    shared.record_upstream_no_healthy_peers("backend");
    shared.record_upstream_bad_gateway_response("backend");

    let snapshot = shared.upstream_stats_snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].downstream_requests_total, 7);
    assert_eq!(snapshot[0].completed_responses_total, 1);
    assert_eq!(snapshot[0].bad_gateway_responses_total, 2);
    assert_eq!(snapshot[0].gateway_timeout_responses_total, 1);
    assert_eq!(snapshot[0].bad_request_responses_total, 1);
    assert_eq!(snapshot[0].payload_too_large_responses_total, 1);
    assert_eq!(snapshot[0].unsupported_media_type_responses_total, 1);
    assert_eq!(snapshot[0].no_healthy_peers_total, 1);
    assert_eq!(snapshot[0].recent_60s.window_secs, 60);
    assert_eq!(snapshot[0].recent_60s.downstream_requests_total, 7);
    assert_eq!(snapshot[0].recent_60s.completed_responses_total, 1);
    assert_eq!(snapshot[0].recent_60s.bad_gateway_responses_total, 2);
    assert_eq!(snapshot[0].recent_60s.gateway_timeout_responses_total, 1);
}
