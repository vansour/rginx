use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rginx_core::{
    ActiveHealthCheck, ConfigSnapshot, Listener, RuntimeSettings, Server, Upstream,
    UpstreamDnsPolicy, UpstreamLoadBalance, UpstreamPeer, UpstreamProtocol, UpstreamSettings,
    UpstreamTls, VirtualHost,
};
use rginx_http::SharedState;

use super::{ProbeKey, collect_probe_targets, initial_probe_delay};

#[tokio::test]
async fn collect_probe_targets_only_includes_enabled_upstreams() {
    let healthy = Arc::new(Upstream::new(
        "healthy".to_string(),
        vec![peer("http://127.0.0.1:9000")],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            active_health_check: Some(ActiveHealthCheck {
                path: "/healthz".to_string(),
                grpc_service: None,
                interval: Duration::from_secs(5),
                timeout: Duration::from_secs(2),
                healthy_successes_required: 2,
            }),
            ..upstream_settings()
        },
    ));
    let passive_only = Arc::new(Upstream::new(
        "passive-only".to_string(),
        vec![peer("http://127.0.0.1:9010")],
        UpstreamTls::NativeRoots,
        UpstreamSettings { ..upstream_settings() },
    ));

    let server = Server {
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
    let snapshot = ConfigSnapshot {
        cache_zones: std::collections::HashMap::new(),
        runtime: RuntimeSettings {
            shutdown_timeout: Duration::from_secs(1),
            worker_threads: None,
            accept_workers: 1,
        },
        listeners: vec![Listener {
            id: "default".to_string(),
            name: "default".to_string(),
            server,
            tls_termination_enabled: false,
            proxy_protocol_enabled: false,
            http3: None,
        }],
        default_vhost: VirtualHost {
            id: "server".to_string(),
            server_names: Vec::new(),
            routes: Vec::new(),
            tls: None,
        },
        vhosts: Vec::new(),
        upstreams: HashMap::from([
            ("healthy".to_string(), healthy),
            ("passive-only".to_string(), passive_only),
        ]),
    };

    let shared = SharedState::from_config(snapshot).expect("shared state should build");
    let targets = collect_probe_targets(shared.snapshot().await);

    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].key.upstream_name, "healthy");
    assert_eq!(targets[0].key.peer_url, "http://127.0.0.1:9000");
    assert_eq!(targets[0].health_check.path, "/healthz");
}

#[test]
fn initial_probe_delay_stays_within_interval_and_is_deterministic() {
    let interval = Duration::from_secs(5);
    let key = ProbeKey {
        upstream_name: "backend".to_string(),
        peer_url: "http://127.0.0.1:9000".to_string(),
    };

    let first = initial_probe_delay(&key, interval);
    let second = initial_probe_delay(&key, interval);

    assert!(first < interval);
    assert_eq!(first, second);
}

#[test]
fn initial_probe_delay_varies_across_probe_targets() {
    let interval = Duration::from_secs(5);
    let first = ProbeKey {
        upstream_name: "backend-a".to_string(),
        peer_url: "http://127.0.0.1:9000".to_string(),
    };
    let second = ProbeKey {
        upstream_name: "backend-b".to_string(),
        peer_url: "http://127.0.0.1:9001".to_string(),
    };

    assert_ne!(initial_probe_delay(&first, interval), initial_probe_delay(&second, interval));
}

fn peer(url: &str) -> UpstreamPeer {
    let (scheme, authority) = url.split_once("://").expect("peer URL should include a scheme");
    UpstreamPeer {
        url: url.to_string(),
        scheme: scheme.to_string(),
        authority: authority.to_string(),
        weight: 1,
        backup: false,
    }
}

fn upstream_settings() -> UpstreamSettings {
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
        unhealthy_after_failures: 2,
        unhealthy_cooldown: Duration::from_secs(10),
        active_health_check: None,
    }
}
