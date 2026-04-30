use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rginx_core::{
    ActiveHealthCheck, Upstream, UpstreamDnsPolicy, UpstreamLoadBalance, UpstreamPeer,
    UpstreamProtocol, UpstreamSettings, UpstreamTls,
};

use super::{PeerHealth, PeerHealthRegistry, UpstreamResolverRuntimeSnapshot};
use crate::proxy::ResolvedUpstreamPeer;

fn resolved_peer(peer: &UpstreamPeer) -> ResolvedUpstreamPeer {
    let socket_addr = peer
        .authority
        .parse::<SocketAddr>()
        .unwrap_or_else(|_| SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 80));
    ResolvedUpstreamPeer {
        url: peer.url.clone(),
        logical_peer_url: peer.url.clone(),
        endpoint_key: peer.url.clone(),
        display_url: peer.url.clone(),
        scheme: peer.scheme.clone(),
        upstream_authority: peer.authority.clone(),
        dial_authority: peer.authority.clone(),
        socket_addr,
        server_name: socket_addr.ip().to_string(),
        weight: peer.weight,
        backup: peer.backup,
    }
}

#[test]
fn get_supports_borrowed_upstream_and_peer_lookups() {
    let expected = Arc::new(PeerHealth::default());
    let registry = PeerHealthRegistry {
        policies: Arc::new(HashMap::new()),
        peers: Arc::new(HashMap::from([(
            "backend".to_string(),
            HashMap::from([("http://127.0.0.1:8080".to_string(), expected.clone())]),
        )])),
        endpoint_peers: Arc::new(std::sync::Mutex::new(HashMap::from([(
            "backend".to_string(),
            HashMap::from([("http://127.0.0.1:8080".to_string(), expected.clone())]),
        )]))),
        notifier: None,
    };

    let actual = registry
        .get_endpoint("backend", "http://127.0.0.1:8080")
        .expect("borrowed lookup should find peer");

    assert!(Arc::ptr_eq(&actual, &expected));

    let guard = registry.track_active_request("backend", "http://127.0.0.1:8080");
    assert_eq!(registry.active_requests("backend", "http://127.0.0.1:8080"), 1);
    drop(guard);
    assert_eq!(registry.active_requests("backend", "http://127.0.0.1:8080"), 0);
}

#[test]
fn snapshot_reports_passive_and_active_health_state() {
    let upstream = Upstream::new(
        "backend".to_string(),
        vec![UpstreamPeer {
            url: "http://127.0.0.1:8080".to_string(),
            scheme: "http".to_string(),
            authority: "127.0.0.1:8080".to_string(),
            weight: 2,
            backup: true,
        }],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            protocol: UpstreamProtocol::Auto,
            load_balance: UpstreamLoadBalance::RoundRobin,
            dns: UpstreamDnsPolicy::default(),
            server_name: true,
            server_name_override: None,
            tls_versions: None,
            server_verify_depth: None,
            server_crl_path: None,
            client_identity: None,
            request_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(30),
            pool_idle_timeout: Some(Duration::from_secs(90)),
            pool_max_idle_per_host: usize::MAX,
            tcp_keepalive: None,
            tcp_nodelay: false,
            http2_keep_alive_interval: None,
            http2_keep_alive_timeout: Duration::from_secs(20),
            http2_keep_alive_while_idle: false,
            max_replayable_request_body_bytes: 64 * 1024,
            unhealthy_after_failures: 1,
            unhealthy_cooldown: Duration::from_secs(30),
            active_health_check: Some(ActiveHealthCheck {
                path: "/healthz".to_string(),
                grpc_service: None,
                interval: Duration::from_secs(5),
                timeout: Duration::from_secs(2),
                healthy_successes_required: 2,
            }),
        },
    );
    let server = rginx_core::Server {
        listen_addr: "127.0.0.1:8080".parse().unwrap(),
        server_header: rginx_core::default_server_header(),
        default_certificate: None,
        trusted_proxies: Vec::new(),
        client_ip_header: None,
        keep_alive: true,
        max_headers: None,
        max_request_body_bytes: None,
        max_connections: None,
        header_read_timeout: None,
        request_body_read_timeout: None,
        response_write_timeout: None,
        access_log_format: None,
        tls: None,
    };
    let config = rginx_core::ConfigSnapshot {
        acme: None,
        managed_certificates: Vec::new(),
        cache_zones: HashMap::new(),
        runtime: rginx_core::RuntimeSettings {
            shutdown_timeout: Duration::from_secs(1),
            worker_threads: None,
            accept_workers: 1,
        },
        listeners: vec![rginx_core::Listener {
            id: "default".to_string(),
            name: "default".to_string(),
            server,
            tls_termination_enabled: false,
            proxy_protocol_enabled: false,
            http3: None,
        }],
        default_vhost: rginx_core::VirtualHost {
            id: "server".to_string(),
            server_names: Vec::new(),
            routes: Vec::new(),
            tls: None,
        },
        vhosts: Vec::new(),
        upstreams: HashMap::from([("backend".to_string(), Arc::new(upstream))]),
    };

    let registry = PeerHealthRegistry::from_config(&config);
    let guard = registry.track_active_request("backend", "http://127.0.0.1:8080");
    let failure = registry.record_failure("backend", "http://127.0.0.1:8080");
    assert!(failure.entered_cooldown);
    assert!(registry.record_active_failure("backend", "http://127.0.0.1:8080"));

    let upstream = config.upstreams["backend"].as_ref();
    let snapshots = [registry.snapshot_for_upstream(
        upstream,
        UpstreamResolverRuntimeSnapshot::default(),
        vec![resolved_peer(&upstream.peers[0])],
    )];
    assert_eq!(snapshots.len(), 1);
    let upstream = &snapshots[0];
    assert_eq!(upstream.upstream_name, "backend");
    assert_eq!(upstream.unhealthy_after_failures, 1);
    assert_eq!(upstream.cooldown_ms, 30_000);
    assert!(upstream.active_health_enabled);
    assert_eq!(upstream.peers.len(), 1);
    let peer = &upstream.peers[0];
    assert_eq!(peer.peer_url, "http://127.0.0.1:8080");
    assert!(peer.backup);
    assert_eq!(peer.weight, 2);
    assert!(!peer.available);
    assert_eq!(peer.passive_consecutive_failures, 1);
    assert!(peer.passive_cooldown_remaining_ms.is_some());
    assert!(peer.passive_pending_recovery);
    assert!(peer.active_unhealthy);
    assert_eq!(peer.active_consecutive_successes, 0);
    assert_eq!(peer.active_requests, 1);

    drop(guard);
}

#[test]
fn track_active_request_only_notifies_when_peer_becomes_active_or_idle() {
    let notifications = Arc::new(AtomicUsize::new(0));
    let registry = PeerHealthRegistry {
        policies: Arc::new(HashMap::new()),
        peers: Arc::new(HashMap::from([(
            "backend".to_string(),
            HashMap::from([("http://127.0.0.1:8080".to_string(), Arc::new(PeerHealth::default()))]),
        )])),
        endpoint_peers: Arc::new(std::sync::Mutex::new(HashMap::from([(
            "backend".to_string(),
            HashMap::from([("http://127.0.0.1:8080".to_string(), Arc::new(PeerHealth::default()))]),
        )]))),
        notifier: Some(Arc::new({
            let notifications = notifications.clone();
            move |_upstream_name| {
                notifications.fetch_add(1, Ordering::Relaxed);
            }
        })),
    };

    let first = registry.track_active_request("backend", "http://127.0.0.1:8080");
    assert_eq!(notifications.load(Ordering::Relaxed), 1);

    let second = registry.track_active_request("backend", "http://127.0.0.1:8080");
    assert_eq!(notifications.load(Ordering::Relaxed), 1);

    drop(first);
    assert_eq!(notifications.load(Ordering::Relaxed), 1);

    drop(second);
    assert_eq!(notifications.load(Ordering::Relaxed), 2);
}
