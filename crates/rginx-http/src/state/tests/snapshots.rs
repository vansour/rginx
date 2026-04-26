use super::*;

#[tokio::test]
async fn wait_for_snapshot_change_returns_after_state_update() {
    let shared = SharedState::from_config(snapshot_with_upstream("127.0.0.1:8080"))
        .expect("shared state should build");

    let since_version = shared.current_snapshot_version();
    let waiter = {
        let shared = shared.clone();
        tokio::spawn(async move {
            shared.wait_for_snapshot_change(since_version, Some(Duration::from_secs(1))).await
        })
    };

    tokio::task::yield_now().await;
    shared.record_upstream_request("backend");

    let changed_version = waiter.await.expect("wait task should complete");
    assert!(changed_version > since_version);
    assert_eq!(changed_version, shared.current_snapshot_version());
}

#[tokio::test]
async fn wait_for_snapshot_change_returns_current_version_after_timeout() {
    let shared =
        SharedState::from_config(snapshot("127.0.0.1:8080")).expect("shared state should build");

    shared.record_reload_success(1, Vec::new());
    let since_version = shared.current_snapshot_version();

    let changed_version =
        shared.wait_for_snapshot_change(since_version, Some(Duration::from_millis(20))).await;

    assert_eq!(changed_version, since_version);
}

#[test]
fn snapshot_delta_since_filters_modules_and_reports_changed_targets() {
    let shared = SharedState::from_config(snapshot_with_routes_and_upstream("127.0.0.1:8080"))
        .expect("shared state should build");

    let since_version = shared.current_snapshot_version();
    shared.record_ocsp_refresh_success("listener:default");
    shared.record_downstream_request("default", "server", Some("server/routes[0]|exact:/"));
    shared.record_upstream_request("backend");

    let delta = shared.snapshot_delta_since(
        since_version,
        Some(&[SnapshotModule::Status, SnapshotModule::Traffic, SnapshotModule::Upstreams]),
        Some(30),
    );

    assert_eq!(
        delta.included_modules,
        vec![SnapshotModule::Status, SnapshotModule::Traffic, SnapshotModule::Upstreams]
    );
    assert_eq!(delta.since_version, since_version);
    assert_eq!(delta.current_snapshot_version, shared.current_snapshot_version());
    assert_eq!(delta.recent_window_secs, Some(30));
    assert!(delta.status_version.expect("status version should be present") > since_version);
    assert!(delta.traffic_version.expect("traffic version should be present") > since_version);
    assert!(delta.upstreams_version.expect("upstream version should be present") > since_version);
    assert_eq!(delta.counters_version, None);
    assert_eq!(delta.peer_health_version, None);
    assert_eq!(delta.status_changed, Some(true));
    assert_eq!(delta.counters_changed, None);
    assert_eq!(delta.traffic_changed, Some(true));
    assert_eq!(delta.traffic_recent_changed, Some(true));
    assert_eq!(delta.peer_health_changed, None);
    assert_eq!(delta.upstreams_changed, Some(true));
    assert_eq!(delta.upstreams_recent_changed, Some(true));
    assert_eq!(delta.changed_listener_ids, Some(vec!["default".to_string()]));
    assert_eq!(delta.changed_vhost_ids, Some(vec!["server".to_string()]));
    assert_eq!(delta.changed_route_ids, Some(vec!["server/routes[0]|exact:/".to_string()]));
    assert_eq!(delta.changed_recent_listener_ids, Some(vec!["default".to_string()]));
    assert_eq!(delta.changed_recent_vhost_ids, Some(vec!["server".to_string()]));
    assert_eq!(delta.changed_recent_route_ids, Some(vec!["server/routes[0]|exact:/".to_string()]));
    assert_eq!(delta.changed_peer_health_upstream_names, None);
    assert_eq!(delta.changed_upstream_names, Some(vec!["backend".to_string()]));
    assert_eq!(delta.changed_recent_upstream_names, Some(vec!["backend".to_string()]));
}

#[test]
fn reload_status_snapshot_tracks_last_success_and_failure() {
    let shared =
        SharedState::from_config(snapshot("127.0.0.1:8080")).expect("shared state should build");

    shared.record_reload_success(2, Vec::new());
    let first = shared.reload_status_snapshot();
    assert_eq!(first.attempts_total, 1);
    assert_eq!(first.successes_total, 1);
    assert_eq!(first.failures_total, 0);
    assert!(matches!(
        first.last_result.as_ref().map(|result| &result.outcome),
        Some(ReloadOutcomeSnapshot::Success { revision: 2 })
    ));

    shared.record_reload_failure("bad config", 2);
    let second = shared.reload_status_snapshot();
    assert_eq!(second.attempts_total, 2);
    assert_eq!(second.successes_total, 1);
    assert_eq!(second.failures_total, 1);
    assert!(matches!(
        second.last_result.as_ref().map(|result| &result.outcome),
        Some(ReloadOutcomeSnapshot::Failure { error }) if error == "bad config"
    ));
    assert_eq!(second.last_result.as_ref().map(|result| result.active_revision), Some(2));
    assert_eq!(
        second.last_result.as_ref().and_then(|result| result.rollback_preserved_revision),
        Some(2)
    );
}
