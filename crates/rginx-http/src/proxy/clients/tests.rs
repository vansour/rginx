use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rginx_core::{
    ActiveHealthCheck, ConfigSnapshot, Listener, RuntimeSettings, Server, Upstream,
    UpstreamDnsPolicy, UpstreamLoadBalance, UpstreamPeer, UpstreamProtocol, UpstreamSettings,
    UpstreamTls, VirtualHost,
};

use super::{
    EndpointClientCache, ProxyClients, UpstreamClientProfile, build_client_for_profile,
    build_hyper_client_for_endpoint, endpoint_client_cache_capacity,
};

#[test]
fn endpoint_client_cache_capacity_is_bounded_by_pool_settings() {
    assert_eq!(endpoint_client_cache_capacity(0), 16);
    assert_eq!(endpoint_client_cache_capacity(4), 16);
    assert_eq!(endpoint_client_cache_capacity(8), 32);
    assert_eq!(endpoint_client_cache_capacity(usize::MAX), 1024);
}

#[test]
fn endpoint_client_cache_evicts_least_recently_used_entry() {
    let proxy_client = http_proxy_client_for_cache_tests();
    let first = socket_addr(1);
    let second = socket_addr(2);
    let third = socket_addr(3);
    let mut cache = EndpointClientCache::new(2);

    cache.insert(first, hyper_client_for_endpoint(&proxy_client, first));
    cache.insert(second, hyper_client_for_endpoint(&proxy_client, second));
    assert!(cache.get(first).is_some());

    cache.insert(third, hyper_client_for_endpoint(&proxy_client, third));

    assert_eq!(cache.entries.len(), 2);
    assert!(cache.entries.contains_key(&first));
    assert!(!cache.entries.contains_key(&second));
    assert!(cache.entries.contains_key(&third));
}

#[tokio::test]
async fn peer_health_snapshot_delegates_to_registry() {
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
        upstreams: HashMap::from([("backend".to_string(), upstream)]),
    };

    let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
    let snapshot = clients.peer_health_snapshot().await;
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].upstream_name, "backend");
    assert_eq!(snapshot[0].peers.len(), 1);
    assert_eq!(snapshot[0].peers[0].peer_url, "http://127.0.0.1:9000");
}

fn http_proxy_client_for_cache_tests() -> Arc<super::HttpProxyClient> {
    let profile = UpstreamClientProfile {
        tls: UpstreamTls::Insecure,
        dns: UpstreamDnsPolicy::default(),
        tls_versions: None,
        server_verify_depth: None,
        server_crl_path: None,
        client_identity: None,
        protocol: UpstreamProtocol::Auto,
        server_name: false,
        server_name_override: None,
        connect_timeout: Duration::from_secs(1),
        pool_idle_timeout: Some(Duration::from_secs(1)),
        pool_max_idle_per_host: 1,
        tcp_keepalive: None,
        tcp_nodelay: true,
        http2_keep_alive_interval: None,
        http2_keep_alive_timeout: Duration::from_secs(20),
        http2_keep_alive_while_idle: false,
    };

    match build_client_for_profile(&profile).expect("proxy client should build") {
        super::ProxyClient::Http(client) => client,
        super::ProxyClient::Http3(_) => panic!("cache test profile should build an HTTP client"),
    }
}

fn hyper_client_for_endpoint(
    proxy_client: &super::HttpProxyClient,
    socket_addr: SocketAddr,
) -> super::HyperProxyClient {
    build_hyper_client_for_endpoint(proxy_client, socket_addr)
        .expect("endpoint hyper client should build")
}

fn socket_addr(last_octet: u8) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, last_octet], 9000))
}
