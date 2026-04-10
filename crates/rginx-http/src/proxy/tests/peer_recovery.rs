use super::*;

#[test]
fn unhealthy_peer_is_skipped_after_consecutive_failures() {
    let snapshot = snapshot_with_upstream_policy(
        "backend",
        vec![peer("http://127.0.0.1:9000"), peer("http://127.0.0.1:9001")],
        2,
        Duration::from_secs(30),
    );
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let first =
        clients.select_peers(snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 2);
    assert_eq!(first.skipped_unhealthy, 0);
    assert_eq!(
        first.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
        vec!["http://127.0.0.1:9000", "http://127.0.0.1:9001"]
    );

    let first_failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
    assert_eq!(first_failure.consecutive_failures, 1);
    assert!(!first_failure.entered_cooldown);

    let second_failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
    assert_eq!(second_failure.consecutive_failures, 2);
    assert!(second_failure.entered_cooldown);

    let selected =
        clients.select_peers(snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 2);
    assert_eq!(selected.skipped_unhealthy, 1);
    assert_eq!(
        selected.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
        vec!["http://127.0.0.1:9001"]
    );
}

#[tokio::test]
async fn unhealthy_peer_recovers_after_cooldown() {
    let snapshot = snapshot_with_upstream_policy(
        "backend",
        vec![peer("http://127.0.0.1:9000"), peer("http://127.0.0.1:9001")],
        1,
        Duration::from_millis(20),
    );
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
    assert!(failure.entered_cooldown);

    let immediately =
        clients.select_peers(snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 2);
    assert_eq!(immediately.skipped_unhealthy, 1);
    assert_eq!(
        immediately.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
        vec!["http://127.0.0.1:9001"]
    );

    tokio::time::sleep(Duration::from_millis(30)).await;

    let recovered =
        clients.select_peers(snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 2);
    assert_eq!(recovered.skipped_unhealthy, 0);
    assert_eq!(recovered.peers.len(), 2);
}

#[test]
fn repeated_failures_during_cooldown_do_not_report_duplicate_transition() {
    let snapshot = snapshot_with_upstream_policy(
        "backend",
        vec![peer("http://127.0.0.1:9000")],
        1,
        Duration::from_secs(30),
    );
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let first_failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
    assert!(first_failure.entered_cooldown);

    let second_failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
    assert!(!second_failure.entered_cooldown);
}

#[tokio::test]
async fn successful_request_after_cooldown_reports_passive_recovery() {
    let snapshot = snapshot_with_upstream_policy(
        "backend",
        vec![peer("http://127.0.0.1:9000")],
        1,
        Duration::from_millis(20),
    );
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
    assert!(failure.entered_cooldown);

    tokio::time::sleep(Duration::from_millis(30)).await;

    assert_eq!(
        clients
            .select_peers(snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 1)
            .peers
            .len(),
        1
    );
    assert!(clients.record_peer_success("backend", "http://127.0.0.1:9000"));
}

#[test]
fn successful_request_resets_peer_failure_count() {
    let snapshot = snapshot_with_upstream_policy(
        "backend",
        vec![peer("http://127.0.0.1:9000")],
        2,
        Duration::from_secs(30),
    );
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
    assert_eq!(failure.consecutive_failures, 1);

    assert!(!clients.record_peer_success("backend", "http://127.0.0.1:9000"));

    let after_reset = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
    assert_eq!(after_reset.consecutive_failures, 1);
    assert!(!after_reset.entered_cooldown);
}

#[test]
fn peer_health_policy_is_applied_per_upstream() {
    let fast_fail = Arc::new(Upstream::new(
        "fast-fail".to_string(),
        vec![peer("http://127.0.0.1:9000")],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            unhealthy_after_failures: 1,
            unhealthy_cooldown: Duration::from_secs(30),
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    ));
    let tolerant = Arc::new(Upstream::new(
        "tolerant".to_string(),
        vec![peer("http://127.0.0.1:9010")],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            unhealthy_after_failures: 3,
            unhealthy_cooldown: Duration::from_secs(30),
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    ));

    let snapshot = snapshot_with_upstreams([
        ("fast-fail".to_string(), fast_fail.clone()),
        ("tolerant".to_string(), tolerant.clone()),
    ]);
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let fast_failure = clients.record_peer_failure("fast-fail", "http://127.0.0.1:9000");
    assert!(fast_failure.entered_cooldown);

    let tolerant_failure = clients.record_peer_failure("tolerant", "http://127.0.0.1:9010");
    assert_eq!(tolerant_failure.consecutive_failures, 1);
    assert!(!tolerant_failure.entered_cooldown);

    let fast_selected = clients.select_peers(
        snapshot.upstreams["fast-fail"].as_ref(),
        client_ip("198.51.100.10"),
        1,
    );
    assert!(fast_selected.peers.is_empty());
    assert_eq!(fast_selected.skipped_unhealthy, 1);

    let tolerant_selected = clients.select_peers(
        snapshot.upstreams["tolerant"].as_ref(),
        client_ip("198.51.100.10"),
        1,
    );
    assert_eq!(tolerant_selected.peers.len(), 1);
    assert_eq!(tolerant_selected.skipped_unhealthy, 0);
}

#[test]
fn active_health_requires_recovery_threshold_before_peer_is_reused() {
    let snapshot =
        snapshot_with_active_health("backend", vec![peer("http://127.0.0.1:9000")], "/healthz", 2);
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    assert_eq!(
        clients
            .select_peers(snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 1)
            .peers
            .len(),
        1
    );
    assert!(clients.record_active_peer_failure("backend", "http://127.0.0.1:9000"));
    assert!(
        clients
            .select_peers(snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 1)
            .peers
            .is_empty()
    );

    let first_success = clients.record_active_peer_success("backend", "http://127.0.0.1:9000", 2);
    assert!(!first_success.recovered);
    assert_eq!(first_success.consecutive_successes, 1);
    assert!(
        clients
            .select_peers(snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 1)
            .peers
            .is_empty()
    );

    let second_success = clients.record_active_peer_success("backend", "http://127.0.0.1:9000", 2);
    assert!(second_success.recovered);
    assert_eq!(second_success.consecutive_successes, 2);
    assert_eq!(
        clients
            .select_peers(snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 1)
            .peers
            .len(),
        1
    );
}

#[tokio::test]
async fn active_health_probe_tracks_status_transitions() {
    let statuses = Arc::new(Mutex::new(VecDeque::from([
        StatusCode::SERVICE_UNAVAILABLE,
        StatusCode::OK,
        StatusCode::OK,
    ])));
    let status_server = spawn_status_server(statuses).await;
    let peer_url = format!("http://{}", status_server.listen_addr);
    let snapshot = snapshot_with_active_health("backend", vec![peer(&peer_url)], "/healthz", 2);
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
    let upstream = snapshot.upstreams["backend"].clone();
    let peer = upstream.peers[0].clone();

    probe_upstream_peer(clients.clone(), upstream.clone(), peer.clone()).await;
    assert!(
        clients.select_peers(upstream.as_ref(), client_ip("198.51.100.10"), 1).peers.is_empty()
    );

    probe_upstream_peer(clients.clone(), upstream.clone(), peer.clone()).await;
    assert!(
        clients.select_peers(upstream.as_ref(), client_ip("198.51.100.10"), 1).peers.is_empty()
    );

    probe_upstream_peer(clients.clone(), upstream.clone(), peer).await;
    assert_eq!(
        clients.select_peers(upstream.as_ref(), client_ip("198.51.100.10"), 1).peers.len(),
        1
    );
}
