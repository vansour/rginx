use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use rginx_core::{
    ActiveHealthCheck, ConfigSnapshot, Listener, RuntimeSettings, Server, Upstream,
    UpstreamLoadBalance, UpstreamPeer, UpstreamProtocol, UpstreamSettings, UpstreamTls,
    VirtualHost,
};

use super::ProxyClients;

#[test]
fn peer_health_snapshot_delegates_to_registry() {
    let upstream = Arc::new(Upstream::new(
        "backend".to_string(),
        vec![UpstreamPeer {
            url: "http://127.0.0.1:9000".to_string(),
            scheme: "http".to_string(),
            authority: "127.0.0.1:9000".to_string(),
            weight: 1,
            backup: false,
        }],
        UpstreamTls::NativeRoots,
        UpstreamSettings {
            protocol: UpstreamProtocol::Auto,
            load_balance: UpstreamLoadBalance::RoundRobin,
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
            active_health_check: Some(ActiveHealthCheck {
                path: "/healthz".to_string(),
                grpc_service: None,
                interval: Duration::from_secs(5),
                timeout: Duration::from_secs(2),
                healthy_successes_required: 2,
            }),
        },
    ));
    let server = Server {
        listen_addr: "127.0.0.1:8080".parse().unwrap(),
        default_certificate: None,
        trusted_proxies: Vec::new(),
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
        }],
        default_vhost: VirtualHost {
            id: "server".to_string(),
            server_names: Vec::new(),
            routes: Vec::new(),
            tls: None,
        },
        vhosts: Vec::new(),
        upstreams: HashMap::from([("backend".to_string(), upstream)]),
    };

    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
    let snapshot = clients.peer_health_snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].upstream_name, "backend");
    assert_eq!(snapshot[0].peers.len(), 1);
    assert_eq!(snapshot[0].peers[0].peer_url, "http://127.0.0.1:9000");
}
