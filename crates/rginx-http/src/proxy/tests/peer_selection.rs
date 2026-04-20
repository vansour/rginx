use super::*;

#[test]
fn upstream_next_peers_returns_distinct_failover_candidates() {
    let upstream = Upstream::new(
        "backend".to_string(),
        vec![
            peer("http://127.0.0.1:9000"),
            peer("http://127.0.0.1:9001"),
            peer("http://127.0.0.1:9002"),
        ],
        UpstreamTls::NativeRoots,
        upstream_settings(UpstreamProtocol::Auto),
    );

    let first = upstream.next_peers(2);
    let second = upstream.next_peers(2);

    assert_eq!(
        first.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
        vec!["http://127.0.0.1:9000", "http://127.0.0.1:9001",]
    );
    assert_eq!(
        second.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
        vec!["http://127.0.0.1:9001", "http://127.0.0.1:9002",]
    );
}

#[test]
fn replayable_idempotent_requests_retry_once() {
    let prepared = PreparedProxyRequest {
        method: Method::GET,
        uri: Uri::from_static("/"),
        headers: HeaderMap::new(),
        body: PreparedRequestBody::Replayable { body: Bytes::new(), trailers: None },
        peer_failover_enabled: true,
        wait_for_streaming_body: false,
    };
    let peers = [peer("http://127.0.0.1:9000"), peer("http://127.0.0.1:9001")];

    assert!(can_retry_peer_request(&prepared, peers.len(), 0));
    assert!(!can_retry_peer_request(&prepared, peers.len(), 1));
}

#[test]
fn streaming_requests_do_not_retry() {
    let prepared = PreparedProxyRequest {
        method: Method::GET,
        uri: Uri::from_static("/"),
        headers: HeaderMap::new(),
        body: PreparedRequestBody::Streaming(None),
        peer_failover_enabled: false,
        wait_for_streaming_body: false,
    };
    let peers = [peer("http://127.0.0.1:9000"), peer("http://127.0.0.1:9001")];

    assert!(!can_retry_peer_request(&prepared, peers.len(), 0));
}

#[test]
fn idempotent_method_detection_matches_retry_policy() {
    assert!(is_idempotent_method(&Method::GET));
    assert!(is_idempotent_method(&Method::PUT));
    assert!(is_idempotent_method(&Method::DELETE));
    assert!(!is_idempotent_method(&Method::POST));
    assert!(!is_idempotent_method(&Method::PATCH));
}

#[tokio::test]
async fn ip_hash_keeps_the_same_primary_peer_for_the_same_client_ip() {
    let upstream = Upstream::new(
        "backend".to_string(),
        vec![
            peer("http://127.0.0.1:9000"),
            peer("http://127.0.0.1:9001"),
            peer("http://127.0.0.1:9002"),
        ],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            load_balance: UpstreamLoadBalance::IpHash,
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );
    let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let first =
        select(&clients, snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 2)
            .await;
    let second =
        select(&clients, snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 2)
            .await;

    assert_eq!(
        first.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
        second.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn ip_hash_skips_unhealthy_primary_and_uses_the_next_peer() {
    let upstream = Upstream::new(
        "backend".to_string(),
        vec![
            peer("http://127.0.0.1:9000"),
            peer("http://127.0.0.1:9001"),
            peer("http://127.0.0.1:9002"),
        ],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            load_balance: UpstreamLoadBalance::IpHash,
            unhealthy_after_failures: 1,
            unhealthy_cooldown: Duration::from_secs(30),
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );
    let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let initial =
        select(&clients, snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 2)
            .await;
    let primary = initial.peers[0].url.clone();
    let fallback = initial.peers[1].url.clone();

    let failure = clients.record_peer_failure("backend", &primary);
    assert!(failure.entered_cooldown);

    let selected =
        select(&clients, snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 2)
            .await;
    assert_eq!(selected.skipped_unhealthy, 1);
    assert_eq!(selected.peers[0].url, fallback);
}

#[tokio::test]
async fn ip_hash_distributes_multiple_client_ips_across_peers() {
    let upstream = Upstream::new(
        "backend".to_string(),
        vec![
            peer("http://127.0.0.1:9000"),
            peer("http://127.0.0.1:9001"),
            peer("http://127.0.0.1:9002"),
        ],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            load_balance: UpstreamLoadBalance::IpHash,
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );
    let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let mut unique_primaries = std::collections::HashSet::new();
    for suffix in 1..=16 {
        let ip = format!("198.51.100.{suffix}");
        unique_primaries.insert(
            select(&clients, snapshot.upstreams["backend"].as_ref(), client_ip(&ip), 1).await.peers
                [0]
            .url
            .clone(),
        );
    }

    assert!(
        unique_primaries.len() >= 2,
        "expected ip_hash to spread clients across at least two peers"
    );
}

#[tokio::test]
async fn weighted_ip_hash_prefers_heavier_peers() {
    let upstream = Upstream::new(
        "backend".to_string(),
        vec![
            peer_with_weight("http://127.0.0.1:9000", 5),
            peer_with_weight("http://127.0.0.1:9001", 1),
        ],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            load_balance: UpstreamLoadBalance::IpHash,
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );
    let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let mut heavy = 0;
    for suffix in 0..=255 {
        let ip = format!("198.51.100.{suffix}");
        if select(&clients, snapshot.upstreams["backend"].as_ref(), client_ip(&ip), 1).await.peers
            [0]
        .url == "http://127.0.0.1:9000"
        {
            heavy += 1;
        }
    }

    assert!(heavy > 128, "expected weighted ip_hash to prefer the heavier peer");
}

#[tokio::test]
async fn backup_peer_is_only_used_as_retry_candidate_while_primary_is_healthy() {
    let upstream = Upstream::new(
        "backend".to_string(),
        vec![peer("http://127.0.0.1:9000"), peer_with_role("http://127.0.0.1:9010", 1, true)],
        UpstreamTls::NativeRoots,
        upstream_settings(UpstreamProtocol::Auto),
    );
    let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let primary_only =
        select(&clients, snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 1)
            .await;
    assert_eq!(
        primary_only.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
        vec!["http://127.0.0.1:9000"]
    );

    let with_retry =
        select(&clients, snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 2)
            .await;
    assert_eq!(
        with_retry.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
        vec!["http://127.0.0.1:9000", "http://127.0.0.1:9010"]
    );
}

#[tokio::test]
async fn backup_peer_is_selected_when_primary_pool_is_unhealthy() {
    let upstream = Upstream::new(
        "backend".to_string(),
        vec![peer("http://127.0.0.1:9000"), peer_with_role("http://127.0.0.1:9010", 1, true)],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            unhealthy_after_failures: 1,
            unhealthy_cooldown: Duration::from_secs(30),
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );
    let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
    assert!(failure.entered_cooldown);

    let selected =
        select(&clients, snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 1)
            .await;
    assert_eq!(selected.skipped_unhealthy, 1);
    assert_eq!(
        selected.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
        vec!["http://127.0.0.1:9010"]
    );
}

#[tokio::test]
async fn least_conn_prefers_peers_with_fewer_active_requests() {
    let upstream = Upstream::new(
        "backend".to_string(),
        vec![
            peer("http://127.0.0.1:9000"),
            peer("http://127.0.0.1:9001"),
            peer("http://127.0.0.1:9002"),
        ],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            load_balance: UpstreamLoadBalance::LeastConn,
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );
    let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let _peer_a_1 = clients.track_active_request("backend", "http://127.0.0.1:9000");
    let _peer_a_2 = clients.track_active_request("backend", "http://127.0.0.1:9000");
    let _peer_b_1 = clients.track_active_request("backend", "http://127.0.0.1:9001");

    let selected =
        select(&clients, snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 3)
            .await;

    assert_eq!(
        selected.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
        vec!["http://127.0.0.1:9002", "http://127.0.0.1:9001", "http://127.0.0.1:9000",]
    );
}

#[tokio::test]
async fn least_conn_uses_configured_peer_order_to_break_ties() {
    let upstream = Upstream::new(
        "backend".to_string(),
        vec![
            peer("http://127.0.0.1:9000"),
            peer("http://127.0.0.1:9001"),
            peer("http://127.0.0.1:9002"),
        ],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            load_balance: UpstreamLoadBalance::LeastConn,
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );
    let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let selected =
        select(&clients, snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 3)
            .await;

    assert_eq!(
        selected.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
        vec!["http://127.0.0.1:9000", "http://127.0.0.1:9001", "http://127.0.0.1:9002",]
    );
}

#[tokio::test]
async fn weighted_least_conn_prefers_higher_capacity_peer_when_projected_load_ties() {
    let upstream = Upstream::new(
        "backend".to_string(),
        vec![
            peer_with_weight("http://127.0.0.1:9000", 3),
            peer_with_weight("http://127.0.0.1:9001", 1),
        ],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            load_balance: UpstreamLoadBalance::LeastConn,
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );
    let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let _peer_a_1 = clients.track_active_request("backend", "http://127.0.0.1:9000");
    let _peer_a_2 = clients.track_active_request("backend", "http://127.0.0.1:9000");

    let selected =
        select(&clients, snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 2)
            .await;

    assert_eq!(
        selected.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
        vec!["http://127.0.0.1:9000", "http://127.0.0.1:9001"]
    );
}

#[tokio::test]
async fn least_conn_ignores_backup_peers_while_primary_pool_is_available() {
    let upstream = Upstream::new(
        "backend".to_string(),
        vec![peer("http://127.0.0.1:9000"), peer_with_role("http://127.0.0.1:9010", 1, true)],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            load_balance: UpstreamLoadBalance::LeastConn,
            ..upstream_settings(UpstreamProtocol::Auto)
        },
    );
    let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

    let selected =
        select(&clients, snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 1)
            .await;
    assert_eq!(
        selected.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
        vec!["http://127.0.0.1:9000"]
    );
}
