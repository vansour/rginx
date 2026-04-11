use std::fs;
use std::time::Duration;

use crate::model::{
    Config, HandlerConfig, ListenerConfig, LocationConfig, MatcherConfig, RuntimeConfig,
    ServerConfig, ServerTlsConfig, TlsCipherSuiteConfig, TlsKeyExchangeGroupConfig, UpstreamConfig,
    UpstreamLoadBalanceConfig, UpstreamPeerConfig, UpstreamProtocolConfig, UpstreamTlsConfig,
    VirtualHostConfig,
};
use tempfile::TempDir;

use super::{
    DEFAULT_HEALTH_CHECK_INTERVAL_SECS, DEFAULT_HEALTH_CHECK_TIMEOUT_SECS,
    DEFAULT_HEALTHY_SUCCESSES_REQUIRED, DEFAULT_MAX_REPLAYABLE_REQUEST_BODY_BYTES,
    DEFAULT_UNHEALTHY_AFTER_FAILURES, DEFAULT_UNHEALTHY_COOLDOWN_SECS,
    DEFAULT_UPSTREAM_HTTP2_KEEP_ALIVE_TIMEOUT_SECS, DEFAULT_UPSTREAM_POOL_IDLE_TIMEOUT_SECS,
    DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS, compile, compile_with_base,
};

fn default_listener_server(snapshot: &rginx_core::ConfigSnapshot) -> &rginx_core::Server {
    &snapshot
        .listeners
        .first()
        .expect("compiled snapshot should contain at least one listener")
        .server
}

fn temp_base_dir(prefix: &str) -> TempDir {
    tempfile::Builder::new().prefix(prefix).tempdir().expect("temp base dir should be created")
}

#[test]
fn compile_accepts_https_upstreams() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "secure-backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://example.com".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::IpHash,
            server_name: None,
            server_name_override: None,
            request_timeout_secs: None,
            connect_timeout_secs: None,
            read_timeout_secs: None,
            write_timeout_secs: None,
            idle_timeout_secs: None,
            pool_idle_timeout_secs: None,
            pool_max_idle_per_host: None,
            tcp_keepalive_secs: None,
            tcp_nodelay: None,
            http2_keep_alive_interval_secs: None,
            http2_keep_alive_timeout_secs: None,
            http2_keep_alive_while_idle: None,
            max_replayable_request_body_bytes: None,
            unhealthy_after_failures: None,
            unhealthy_cooldown_secs: None,
            health_check_path: Some("/healthz".to_string()),
            health_check_grpc_service: None,
            health_check_interval_secs: None,
            health_check_timeout_secs: None,
            healthy_successes_required: None,
        }],
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Prefix("/api".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "secure-backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("https upstream should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    let peer = proxy.upstream.next_peer().expect("expected one upstream peer");
    assert_eq!(proxy.upstream_name, "secure-backend");
    assert_eq!(peer.scheme, "https");
    assert_eq!(peer.authority, "example.com");
    assert_eq!(proxy.upstream.protocol, rginx_core::UpstreamProtocol::Auto);
    assert_eq!(proxy.upstream.load_balance, rginx_core::UpstreamLoadBalance::IpHash);
    assert_eq!(
        proxy.upstream.request_timeout,
        Duration::from_secs(DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS)
    );
    assert_eq!(
        proxy.upstream.max_replayable_request_body_bytes,
        DEFAULT_MAX_REPLAYABLE_REQUEST_BODY_BYTES as usize
    );
    assert_eq!(proxy.upstream.unhealthy_after_failures, DEFAULT_UNHEALTHY_AFTER_FAILURES);
    assert_eq!(
        proxy.upstream.unhealthy_cooldown,
        Duration::from_secs(DEFAULT_UNHEALTHY_COOLDOWN_SECS)
    );
    let active_health = proxy
        .upstream
        .active_health_check
        .as_ref()
        .expect("active health-check config should compile");
    assert_eq!(active_health.path, "/healthz");
    assert_eq!(active_health.grpc_service, None);
    assert_eq!(active_health.interval, Duration::from_secs(DEFAULT_HEALTH_CHECK_INTERVAL_SECS));
    assert_eq!(active_health.timeout, Duration::from_secs(DEFAULT_HEALTH_CHECK_TIMEOUT_SECS));
    assert_eq!(active_health.healthy_successes_required, DEFAULT_HEALTHY_SUCCESSES_REQUIRED);
}

#[test]
fn compile_defaults_grpc_health_check_path_when_service_is_set() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "grpc-backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://example.com".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::RoundRobin,
            server_name: None,
            server_name_override: None,
            request_timeout_secs: None,
            connect_timeout_secs: None,
            read_timeout_secs: None,
            write_timeout_secs: None,
            idle_timeout_secs: None,
            pool_idle_timeout_secs: None,
            pool_max_idle_per_host: None,
            tcp_keepalive_secs: None,
            tcp_nodelay: None,
            http2_keep_alive_interval_secs: None,
            http2_keep_alive_timeout_secs: None,
            http2_keep_alive_while_idle: None,
            max_replayable_request_body_bytes: None,
            unhealthy_after_failures: None,
            unhealthy_cooldown_secs: None,
            health_check_path: None,
            health_check_grpc_service: Some("grpc.health.v1.Health".to_string()),
            health_check_interval_secs: None,
            health_check_timeout_secs: None,
            healthy_successes_required: None,
        }],
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "grpc-backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("gRPC health-check config should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    let active_health = proxy
        .upstream
        .active_health_check
        .as_ref()
        .expect("gRPC active health-check config should compile");
    assert_eq!(active_health.path, super::DEFAULT_GRPC_HEALTH_CHECK_PATH);
    assert_eq!(active_health.grpc_service.as_deref(), Some("grpc.health.v1.Health"));
}

#[test]
fn compile_applies_granular_upstream_transport_settings() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://example.com".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::IpHash,
            server_name: None,
            server_name_override: None,
            request_timeout_secs: None,
            connect_timeout_secs: Some(3),
            read_timeout_secs: Some(4),
            write_timeout_secs: Some(5),
            idle_timeout_secs: Some(6),
            pool_idle_timeout_secs: Some(7),
            pool_max_idle_per_host: Some(8),
            tcp_keepalive_secs: Some(9),
            tcp_nodelay: Some(true),
            http2_keep_alive_interval_secs: Some(10),
            http2_keep_alive_timeout_secs: Some(11),
            http2_keep_alive_while_idle: Some(true),
            max_replayable_request_body_bytes: None,
            unhealthy_after_failures: None,
            unhealthy_cooldown_secs: None,
            health_check_path: None,
            health_check_grpc_service: None,
            health_check_interval_secs: None,
            health_check_timeout_secs: None,
            healthy_successes_required: None,
        }],
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("granular upstream settings should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    assert_eq!(proxy.upstream.load_balance, rginx_core::UpstreamLoadBalance::IpHash);
    assert_eq!(proxy.upstream.connect_timeout, Duration::from_secs(3));
    assert_eq!(proxy.upstream.request_timeout, Duration::from_secs(4));
    assert_eq!(proxy.upstream.write_timeout, Duration::from_secs(5));
    assert_eq!(proxy.upstream.idle_timeout, Duration::from_secs(6));
    assert_eq!(proxy.upstream.pool_idle_timeout, Some(Duration::from_secs(7)));
    assert_eq!(proxy.upstream.pool_max_idle_per_host, 8);
    assert_eq!(proxy.upstream.tcp_keepalive, Some(Duration::from_secs(9)));
    assert!(proxy.upstream.tcp_nodelay);
    assert_eq!(proxy.upstream.http2_keep_alive_interval, Some(Duration::from_secs(10)));
    assert_eq!(proxy.upstream.http2_keep_alive_timeout, Duration::from_secs(11));
    assert!(proxy.upstream.http2_keep_alive_while_idle);
}

#[test]
fn compile_accepts_least_conn_load_balance() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "backend".to_string(),
            peers: vec![
                UpstreamPeerConfig {
                    url: "http://127.0.0.1:9000".to_string(),
                    weight: 1,
                    backup: false,
                },
                UpstreamPeerConfig {
                    url: "http://127.0.0.1:9001".to_string(),
                    weight: 1,
                    backup: false,
                },
            ],
            tls: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::LeastConn,
            server_name: None,
            server_name_override: None,
            request_timeout_secs: None,
            connect_timeout_secs: None,
            read_timeout_secs: None,
            write_timeout_secs: None,
            idle_timeout_secs: None,
            pool_idle_timeout_secs: None,
            pool_max_idle_per_host: None,
            tcp_keepalive_secs: None,
            tcp_nodelay: None,
            http2_keep_alive_interval_secs: None,
            http2_keep_alive_timeout_secs: None,
            http2_keep_alive_while_idle: None,
            max_replayable_request_body_bytes: None,
            unhealthy_after_failures: None,
            unhealthy_cooldown_secs: None,
            health_check_path: None,
            health_check_grpc_service: None,
            health_check_interval_secs: None,
            health_check_timeout_secs: None,
            healthy_successes_required: None,
        }],
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("least_conn config should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    assert_eq!(proxy.upstream.load_balance, rginx_core::UpstreamLoadBalance::LeastConn);
}

#[test]
fn compile_applies_peer_weights() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "backend".to_string(),
            peers: vec![
                UpstreamPeerConfig {
                    url: "http://127.0.0.1:9000".to_string(),
                    weight: 3,
                    backup: false,
                },
                UpstreamPeerConfig {
                    url: "http://127.0.0.1:9001".to_string(),
                    weight: 1,
                    backup: false,
                },
            ],
            tls: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::RoundRobin,
            server_name: None,
            server_name_override: None,
            request_timeout_secs: None,
            connect_timeout_secs: None,
            read_timeout_secs: None,
            write_timeout_secs: None,
            idle_timeout_secs: None,
            pool_idle_timeout_secs: None,
            pool_max_idle_per_host: None,
            tcp_keepalive_secs: None,
            tcp_nodelay: None,
            http2_keep_alive_interval_secs: None,
            http2_keep_alive_timeout_secs: None,
            http2_keep_alive_while_idle: None,
            max_replayable_request_body_bytes: None,
            unhealthy_after_failures: None,
            unhealthy_cooldown_secs: None,
            health_check_path: None,
            health_check_grpc_service: None,
            health_check_interval_secs: None,
            health_check_timeout_secs: None,
            healthy_successes_required: None,
        }],
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("weighted peer config should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    assert_eq!(proxy.upstream.peers[0].weight, 3);
    assert_eq!(proxy.upstream.peers[1].weight, 1);

    let observed = (0..4)
        .map(|_| proxy.upstream.next_peer().expect("expected weighted peer").url.clone())
        .collect::<Vec<_>>();

    assert_eq!(
        observed,
        vec![
            "http://127.0.0.1:9000".to_string(),
            "http://127.0.0.1:9000".to_string(),
            "http://127.0.0.1:9000".to_string(),
            "http://127.0.0.1:9001".to_string(),
        ]
    );
}

#[test]
fn compile_accepts_backup_peers() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "backend".to_string(),
            peers: vec![
                UpstreamPeerConfig {
                    url: "http://127.0.0.1:9000".to_string(),
                    weight: 1,
                    backup: false,
                },
                UpstreamPeerConfig {
                    url: "http://127.0.0.1:9001".to_string(),
                    weight: 1,
                    backup: true,
                },
            ],
            tls: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::RoundRobin,
            server_name: None,
            server_name_override: None,
            request_timeout_secs: None,
            connect_timeout_secs: None,
            read_timeout_secs: None,
            write_timeout_secs: None,
            idle_timeout_secs: None,
            pool_idle_timeout_secs: None,
            pool_max_idle_per_host: None,
            tcp_keepalive_secs: None,
            tcp_nodelay: None,
            http2_keep_alive_interval_secs: None,
            http2_keep_alive_timeout_secs: None,
            http2_keep_alive_while_idle: None,
            max_replayable_request_body_bytes: None,
            unhealthy_after_failures: None,
            unhealthy_cooldown_secs: None,
            health_check_path: None,
            health_check_grpc_service: None,
            health_check_interval_secs: None,
            health_check_timeout_secs: None,
            healthy_successes_required: None,
        }],
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("backup peer config should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    assert!(!proxy.upstream.peers[0].backup);
    assert!(proxy.upstream.peers[1].backup);
    assert_eq!(
        proxy.upstream.next_peer().expect("primary peer should be selected").url,
        "http://127.0.0.1:9000"
    );
    assert_eq!(
        proxy
            .upstream
            .backup_next_peers(1)
            .into_iter()
            .next()
            .expect("backup peer should be available")
            .url,
        "http://127.0.0.1:9001"
    );
}

#[test]
fn compile_uses_legacy_request_timeout_fallbacks_and_disables_pool_idle_timeout() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://example.com".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::RoundRobin,
            server_name: None,
            server_name_override: None,
            request_timeout_secs: Some(12),
            connect_timeout_secs: None,
            read_timeout_secs: None,
            write_timeout_secs: None,
            idle_timeout_secs: None,
            pool_idle_timeout_secs: Some(0),
            pool_max_idle_per_host: None,
            tcp_keepalive_secs: None,
            tcp_nodelay: None,
            http2_keep_alive_interval_secs: None,
            http2_keep_alive_timeout_secs: None,
            http2_keep_alive_while_idle: None,
            max_replayable_request_body_bytes: None,
            unhealthy_after_failures: None,
            unhealthy_cooldown_secs: None,
            health_check_path: None,
            health_check_grpc_service: None,
            health_check_interval_secs: None,
            health_check_timeout_secs: None,
            healthy_successes_required: None,
        }],
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("legacy request_timeout_secs should still compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    assert_eq!(proxy.upstream.request_timeout, Duration::from_secs(12));
    assert_eq!(proxy.upstream.connect_timeout, Duration::from_secs(12));
    assert_eq!(proxy.upstream.write_timeout, Duration::from_secs(12));
    assert_eq!(proxy.upstream.idle_timeout, Duration::from_secs(12));
    assert_eq!(proxy.upstream.pool_idle_timeout, None);
    assert_eq!(proxy.upstream.pool_max_idle_per_host, usize::MAX);
    assert_eq!(
        proxy.upstream.http2_keep_alive_timeout,
        Duration::from_secs(DEFAULT_UPSTREAM_HTTP2_KEEP_ALIVE_TIMEOUT_SECS)
    );
}

#[test]
fn compile_uses_default_pool_idle_timeout() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://example.com".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::RoundRobin,
            server_name: None,
            server_name_override: None,
            request_timeout_secs: None,
            connect_timeout_secs: None,
            read_timeout_secs: None,
            write_timeout_secs: None,
            idle_timeout_secs: None,
            pool_idle_timeout_secs: None,
            pool_max_idle_per_host: None,
            tcp_keepalive_secs: None,
            tcp_nodelay: None,
            http2_keep_alive_interval_secs: None,
            http2_keep_alive_timeout_secs: None,
            http2_keep_alive_while_idle: None,
            max_replayable_request_body_bytes: None,
            unhealthy_after_failures: None,
            unhealthy_cooldown_secs: None,
            health_check_path: None,
            health_check_grpc_service: None,
            health_check_interval_secs: None,
            health_check_timeout_secs: None,
            healthy_successes_required: None,
        }],
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("defaults should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    assert_eq!(
        proxy.upstream.pool_idle_timeout,
        Some(Duration::from_secs(DEFAULT_UPSTREAM_POOL_IDLE_TIMEOUT_SECS))
    );
}

#[test]
fn compile_resolves_custom_ca_relative_to_config_base() {
    let base_dir = temp_base_dir("rginx-config-test-");
    let ca_path = base_dir.path().join("dev-ca.pem");
    fs::write(&ca_path, b"placeholder").expect("temp CA file should be written");

    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "dev-backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://localhost:9443".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: Some(UpstreamTlsConfig {
                verify: crate::model::UpstreamTlsModeConfig::CustomCa {
                    ca_cert_path: "dev-ca.pem".to_string(),
                },
                versions: Some(vec![crate::model::TlsVersionConfig::Tls13]),
                verify_depth: None,
                crl_path: None,
                client_cert_path: None,
                client_key_path: None,
            }),
            protocol: UpstreamProtocolConfig::Http2,
            load_balance: UpstreamLoadBalanceConfig::RoundRobin,
            server_name: None,
            server_name_override: Some("dev.internal".to_string()),
            request_timeout_secs: Some(5),
            connect_timeout_secs: None,
            read_timeout_secs: None,
            write_timeout_secs: None,
            idle_timeout_secs: None,
            pool_idle_timeout_secs: None,
            pool_max_idle_per_host: None,
            tcp_keepalive_secs: None,
            tcp_nodelay: None,
            http2_keep_alive_interval_secs: None,
            http2_keep_alive_timeout_secs: None,
            http2_keep_alive_while_idle: None,
            max_replayable_request_body_bytes: Some(1024),
            unhealthy_after_failures: Some(3),
            unhealthy_cooldown_secs: Some(15),
            health_check_path: Some("/ready".to_string()),
            health_check_grpc_service: None,
            health_check_interval_secs: Some(7),
            health_check_timeout_secs: Some(3),
            healthy_successes_required: Some(4),
        }],
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "dev-backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot =
        compile_with_base(config, base_dir.path()).expect("custom CA config should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    assert!(matches!(
        &proxy.upstream.tls,
        rginx_core::UpstreamTls::CustomCa { ca_cert_path } if ca_cert_path == &ca_path
    ));
    assert_eq!(proxy.upstream.protocol, rginx_core::UpstreamProtocol::Http2);
    assert_eq!(proxy.upstream.server_name_override.as_deref(), Some("dev.internal"));
    assert_eq!(proxy.upstream.request_timeout, Duration::from_secs(5));
    assert_eq!(proxy.upstream.max_replayable_request_body_bytes, 1024);
    assert_eq!(proxy.upstream.unhealthy_after_failures, 3);
    assert_eq!(proxy.upstream.unhealthy_cooldown, Duration::from_secs(15));
    let active_health = proxy
        .upstream
        .active_health_check
        .as_ref()
        .expect("custom active health-check config should compile");
    assert_eq!(active_health.path, "/ready");
    assert_eq!(active_health.grpc_service, None);
    assert_eq!(active_health.interval, Duration::from_secs(7));
    assert_eq!(active_health.timeout, Duration::from_secs(3));
    assert_eq!(active_health.healthy_successes_required, 4);
}

#[test]
fn compile_resolves_upstream_mtls_identity_and_tls_versions_relative_to_config_base() {
    let base_dir = temp_base_dir("rginx-upstream-mtls-config-test-");
    let ca_path = base_dir.path().join("upstream-ca.pem");
    let client_cert_path = base_dir.path().join("client.crt");
    let client_key_path = base_dir.path().join("client.key");
    fs::write(&ca_path, b"placeholder").expect("temp CA file should be written");
    fs::write(&client_cert_path, b"placeholder").expect("temp client cert file should be written");
    fs::write(&client_key_path, b"placeholder").expect("temp client key file should be written");

    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "mtls-backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://localhost:9443".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: Some(UpstreamTlsConfig {
                verify: crate::model::UpstreamTlsModeConfig::CustomCa {
                    ca_cert_path: "upstream-ca.pem".to_string(),
                },
                versions: Some(vec![crate::model::TlsVersionConfig::Tls13]),
                verify_depth: None,
                crl_path: None,
                client_cert_path: Some("client.crt".to_string()),
                client_key_path: Some("client.key".to_string()),
            }),
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::RoundRobin,
            server_name: None,
            server_name_override: Some("localhost".to_string()),
            request_timeout_secs: None,
            connect_timeout_secs: None,
            read_timeout_secs: None,
            write_timeout_secs: None,
            idle_timeout_secs: None,
            pool_idle_timeout_secs: None,
            pool_max_idle_per_host: None,
            tcp_keepalive_secs: None,
            tcp_nodelay: None,
            http2_keep_alive_interval_secs: None,
            http2_keep_alive_timeout_secs: None,
            http2_keep_alive_while_idle: None,
            max_replayable_request_body_bytes: None,
            unhealthy_after_failures: None,
            unhealthy_cooldown_secs: None,
            health_check_path: None,
            health_check_grpc_service: None,
            health_check_interval_secs: None,
            health_check_timeout_secs: None,
            healthy_successes_required: None,
        }],
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "mtls-backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: None,
            grpc_method: None,
            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot =
        compile_with_base(config, base_dir.path()).expect("upstream mTLS config should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    assert!(matches!(
        &proxy.upstream.tls,
        rginx_core::UpstreamTls::CustomCa { ca_cert_path } if ca_cert_path == &ca_path
    ));
    assert!(matches!(
        proxy.upstream.tls_versions.as_deref(),
        Some([rginx_core::TlsVersion::Tls13])
    ));
    let client_identity =
        proxy.upstream.client_identity.as_ref().expect("client identity should compile");
    assert_eq!(client_identity.cert_path, client_cert_path);
    assert_eq!(client_identity.key_path, client_key_path);
}

#[test]
fn compile_normalizes_server_name_override() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "secure-backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://[::1]:9443".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::RoundRobin,
            server_name: None,
            server_name_override: Some("[::1]".to_string()),
            request_timeout_secs: None,
            connect_timeout_secs: None,
            read_timeout_secs: None,
            write_timeout_secs: None,
            idle_timeout_secs: None,
            pool_idle_timeout_secs: None,
            pool_max_idle_per_host: None,
            tcp_keepalive_secs: None,
            tcp_nodelay: None,
            http2_keep_alive_interval_secs: None,
            http2_keep_alive_timeout_secs: None,
            http2_keep_alive_while_idle: None,
            max_replayable_request_body_bytes: None,
            unhealthy_after_failures: None,
            unhealthy_cooldown_secs: None,
            health_check_path: None,
            health_check_grpc_service: None,
            health_check_interval_secs: None,
            health_check_timeout_secs: None,
            healthy_successes_required: None,
        }],
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "secure-backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("server name override should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    assert_eq!(proxy.upstream.server_name_override.as_deref(), Some("::1"));
}

#[test]
fn compile_preserves_upstream_server_name_toggle() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "secure-backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://127.0.0.1:9443".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: Some(crate::model::UpstreamTlsConfig {
                verify: crate::model::UpstreamTlsModeConfig::Insecure,
                versions: None,
                verify_depth: None,
                crl_path: None,
                client_cert_path: None,
                client_key_path: None,
            }),
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::RoundRobin,
            server_name: Some(false),
            server_name_override: Some("localhost".to_string()),
            request_timeout_secs: None,
            connect_timeout_secs: None,
            read_timeout_secs: None,
            write_timeout_secs: None,
            idle_timeout_secs: None,
            pool_idle_timeout_secs: None,
            pool_max_idle_per_host: None,
            tcp_keepalive_secs: None,
            tcp_nodelay: None,
            http2_keep_alive_interval_secs: None,
            http2_keep_alive_timeout_secs: None,
            http2_keep_alive_while_idle: None,
            max_replayable_request_body_bytes: None,
            unhealthy_after_failures: None,
            unhealthy_cooldown_secs: None,
            health_check_path: None,
            health_check_grpc_service: None,
            health_check_interval_secs: None,
            health_check_timeout_secs: None,
            healthy_successes_required: None,
        }],
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "secure-backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: None,
            grpc_method: None,
            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("upstream server_name toggle should compile");
    let proxy = match &snapshot.default_vhost.routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => proxy,
        _ => panic!("expected proxy route"),
    };

    assert!(!proxy.upstream.server_name);
    assert_eq!(proxy.upstream.server_name_override.as_deref(), Some("localhost"));
}

#[test]
fn compile_rejects_invalid_server_name_override() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "secure-backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "https://127.0.0.1:9443".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: None,
            protocol: UpstreamProtocolConfig::Auto,
            load_balance: UpstreamLoadBalanceConfig::RoundRobin,
            server_name: None,
            server_name_override: Some("bad name".to_string()),
            request_timeout_secs: None,
            connect_timeout_secs: None,
            read_timeout_secs: None,
            write_timeout_secs: None,
            idle_timeout_secs: None,
            pool_idle_timeout_secs: None,
            pool_max_idle_per_host: None,
            tcp_keepalive_secs: None,
            tcp_nodelay: None,
            http2_keep_alive_interval_secs: None,
            http2_keep_alive_timeout_secs: None,
            http2_keep_alive_while_idle: None,
            max_replayable_request_body_bytes: None,
            unhealthy_after_failures: None,
            unhealthy_cooldown_secs: None,
            health_check_path: None,
            health_check_grpc_service: None,
            health_check_interval_secs: None,
            health_check_timeout_secs: None,
            healthy_successes_required: None,
        }],
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Prefix("/".to_string()),
            handler: HandlerConfig::Proxy {
                upstream: "secure-backend".to_string(),
                preserve_host: None,
                strip_prefix: None,
                proxy_set_headers: std::collections::HashMap::new(),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let error = compile(config).expect_err("invalid override should be rejected");
    assert!(error.to_string().contains("server_name_override"));
}

#[test]
fn compile_attaches_route_access_control() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: Vec::new(),
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Exact("/".to_string()),
            handler: HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("ok\n".to_string()),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: vec!["127.0.0.1/32".to_string(), "::1/128".to_string()],
            deny_cidrs: vec!["127.0.0.2/32".to_string()],
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("access-controlled route should compile");
    assert_eq!(snapshot.default_vhost.routes[0].access_control.allow_cidrs.len(), 2);
    assert_eq!(snapshot.default_vhost.routes[0].access_control.deny_cidrs.len(), 1);
}

#[test]
fn compile_attaches_route_rate_limit() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: Vec::new(),
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Prefix("/api".to_string()),
            handler: HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("ok\n".to_string()),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: Some(20),
            burst: Some(5),
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("rate-limited route should compile");
    let rate_limit =
        snapshot.default_vhost.routes[0].rate_limit.expect("route rate limit should exist");
    assert_eq!(rate_limit.requests_per_sec, 20);
    assert_eq!(rate_limit.burst, 5);
}

#[test]
fn compile_generates_distinct_route_and_vhost_ids() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: vec!["default.example.com".to_string()],
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: Vec::new(),
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Exact("/".to_string()),
            handler: HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("default site\n".to_string()),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: vec![VirtualHostConfig {
            server_names: vec!["api.example.com".to_string()],
            locations: vec![LocationConfig {
                matcher: MatcherConfig::Exact("/".to_string()),
                handler: HandlerConfig::Return {
                    status: 200,
                    location: String::new(),
                    body: Some("api site\n".to_string()),
                },
                grpc_service: None,

                grpc_method: None,

                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: None,
                burst: None,
            }],
            tls: None,
        }],
    };

    let snapshot = compile(config).expect("vhost config should compile");

    assert_eq!(snapshot.default_vhost.id, "server");
    assert_eq!(snapshot.vhosts[0].id, "servers[0]");
    assert_eq!(snapshot.default_vhost.routes[0].id, "server/routes[0]|exact:/");
    assert_eq!(snapshot.vhosts[0].routes[0].id, "servers[0]/routes[0]|exact:/");
    assert_eq!(snapshot.total_vhost_count(), 2);
    assert_eq!(snapshot.total_route_count(), 2);
}

#[test]
fn compile_resolves_server_tls_paths_relative_to_config_base() {
    let base_dir = temp_base_dir("rginx-server-tls-config-test-");
    let cert_path = base_dir.path().join("server.crt");
    let key_path = base_dir.path().join("server.key");
    fs::write(&cert_path, b"placeholder").expect("temp cert file should be written");
    fs::write(&key_path, b"placeholder").expect("temp key file should be written");

    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: Some(ServerTlsConfig {
                cert_path: "server.crt".to_string(),
                key_path: "server.key".to_string(),
                additional_certificates: None,
                versions: None,
                cipher_suites: None,
                key_exchange_groups: None,
                alpn_protocols: None,
                ocsp_staple_path: None,
                ocsp: None,
                session_resumption: None,
                session_tickets: None,
                session_cache_size: None,
                session_ticket_count: None,
                client_auth: None,
            }),
        },
        upstreams: Vec::new(),
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Exact("/".to_string()),
            handler: HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("ok\n".to_string()),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile_with_base(config, base_dir.path()).expect("server TLS should compile");
    let tls =
        default_listener_server(&snapshot).tls.clone().expect("compiled server TLS should exist");
    assert_eq!(tls.cert_path, cert_path);
    assert_eq!(tls.key_path, key_path);
}

#[test]
fn compile_preserves_server_tls_policy_fields() {
    let base_dir = temp_base_dir("rginx-server-tls-policy-test-");
    let cert_path = base_dir.path().join("server.crt");
    let key_path = base_dir.path().join("server.key");
    fs::write(&cert_path, b"placeholder").expect("temp cert file should be written");
    fs::write(&key_path, b"placeholder").expect("temp key file should be written");

    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: Some(ServerTlsConfig {
                cert_path: "server.crt".to_string(),
                key_path: "server.key".to_string(),
                additional_certificates: None,
                versions: Some(vec![crate::model::TlsVersionConfig::Tls13]),
                cipher_suites: Some(vec![TlsCipherSuiteConfig::Tls13Aes128GcmSha256]),
                key_exchange_groups: Some(vec![TlsKeyExchangeGroupConfig::Secp256r1]),
                alpn_protocols: Some(vec!["http/1.1".to_string()]),
                ocsp_staple_path: None,
                ocsp: None,
                session_resumption: Some(true),
                session_tickets: Some(false),
                session_cache_size: Some(512),
                session_ticket_count: None,
                client_auth: None,
            }),
        },
        upstreams: Vec::new(),
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Exact("/".to_string()),
            handler: HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("ok\n".to_string()),
            },
            grpc_service: None,
            grpc_method: None,
            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile_with_base(config, base_dir.path()).expect("server TLS should compile");
    let tls =
        default_listener_server(&snapshot).tls.clone().expect("compiled server TLS should exist");
    assert_eq!(tls.versions, Some(vec![rginx_core::TlsVersion::Tls13]));
    assert_eq!(tls.cipher_suites, Some(vec![rginx_core::TlsCipherSuite::Tls13Aes128GcmSha256]));
    assert_eq!(tls.key_exchange_groups, Some(vec![rginx_core::TlsKeyExchangeGroup::Secp256r1]));
    assert_eq!(tls.alpn_protocols, Some(vec!["http/1.1".to_string()]));
    assert_eq!(tls.session_resumption, Some(true));
    assert_eq!(tls.session_tickets, Some(false));
    assert_eq!(tls.session_cache_size, Some(512));
    assert_eq!(tls.session_ticket_count, None);
}

#[test]
fn compile_preserves_server_tls_ocsp_policy_fields() {
    let base_dir = temp_base_dir("rginx-server-ocsp-policy-test-");
    let cert_path = base_dir.path().join("server.crt");
    let key_path = base_dir.path().join("server.key");
    let ocsp_path = base_dir.path().join("server.ocsp");
    fs::write(&cert_path, b"placeholder").expect("temp cert file should be written");
    fs::write(&key_path, b"placeholder").expect("temp key file should be written");
    fs::write(&ocsp_path, b"").expect("temp ocsp file should be written");

    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: Some(ServerTlsConfig {
                cert_path: "server.crt".to_string(),
                key_path: "server.key".to_string(),
                additional_certificates: None,
                versions: None,
                cipher_suites: None,
                key_exchange_groups: None,
                alpn_protocols: None,
                ocsp_staple_path: Some("server.ocsp".to_string()),
                ocsp: Some(crate::model::OcspConfig {
                    nonce: Some(crate::model::OcspNonceModeConfig::Required),
                    responder_policy: Some(crate::model::OcspResponderPolicyConfig::IssuerOnly),
                }),
                session_resumption: None,
                session_tickets: None,
                session_cache_size: None,
                session_ticket_count: None,
                client_auth: None,
            }),
        },
        upstreams: Vec::new(),
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Exact("/".to_string()),
            handler: HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("ok\n".to_string()),
            },
            grpc_service: None,
            grpc_method: None,
            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile_with_base(config, base_dir.path()).expect("server TLS should compile");
    let tls =
        default_listener_server(&snapshot).tls.clone().expect("compiled server TLS should exist");
    assert_eq!(tls.ocsp.nonce, rginx_core::OcspNonceMode::Required);
    assert_eq!(tls.ocsp.responder_policy, rginx_core::OcspResponderPolicy::IssuerOnly);
}

#[test]
fn compile_normalizes_trusted_proxy_ips_and_cidrs() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: vec!["10.0.0.0/8".to_string(), "127.0.0.1".to_string()],
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: Vec::new(),
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Exact("/".to_string()),
            handler: HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("ok\n".to_string()),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("trusted proxies should compile");
    assert_eq!(default_listener_server(&snapshot).trusted_proxies.len(), 2);
    assert!(default_listener_server(&snapshot).is_trusted_proxy("10.1.2.3".parse().unwrap()));
    assert!(default_listener_server(&snapshot).is_trusted_proxy("127.0.0.1".parse().unwrap()));
}

#[test]
fn compile_attaches_server_hardening_settings() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: Some(3),
            accept_workers: Some(2),
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: Some(false),
            max_headers: Some(32),
            max_request_body_bytes: Some(1024),
            max_connections: Some(256),
            header_read_timeout_secs: Some(3),
            request_body_read_timeout_secs: Some(4),
            response_write_timeout_secs: Some(5),
            access_log_format: Some("$request_id $status $request".to_string()),
            tls: None,
        },
        upstreams: Vec::new(),
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Exact("/".to_string()),
            handler: HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("ok\n".to_string()),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("server hardening settings should compile");
    assert_eq!(snapshot.runtime.worker_threads, Some(3));
    assert_eq!(snapshot.runtime.accept_workers, 2);
    assert!(!default_listener_server(&snapshot).keep_alive);
    assert_eq!(default_listener_server(&snapshot).max_headers, Some(32));
    assert_eq!(default_listener_server(&snapshot).max_request_body_bytes, Some(1024));
    assert_eq!(default_listener_server(&snapshot).max_connections, Some(256));
    assert_eq!(
        default_listener_server(&snapshot).header_read_timeout,
        Some(Duration::from_secs(3))
    );
    assert_eq!(
        default_listener_server(&snapshot).request_body_read_timeout,
        Some(Duration::from_secs(4))
    );
    assert_eq!(
        default_listener_server(&snapshot).response_write_timeout,
        Some(Duration::from_secs(5))
    );
    let access_log_format = default_listener_server(&snapshot)
        .access_log_format
        .as_ref()
        .expect("access log format should compile");
    assert_eq!(access_log_format.template(), "$request_id $status $request");
}

#[test]
fn compile_prioritizes_grpc_constrained_routes_with_same_path_matcher() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: Vec::new(),
        locations: vec![
            LocationConfig {
                matcher: MatcherConfig::Prefix("/".to_string()),
                handler: HandlerConfig::Return {
                    status: 200,
                    location: String::new(),
                    body: Some("fallback\n".to_string()),
                },
                grpc_service: None,
                grpc_method: None,
                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: None,
                burst: None,
            },
            LocationConfig {
                matcher: MatcherConfig::Prefix("/".to_string()),
                handler: HandlerConfig::Return {
                    status: 200,
                    location: String::new(),
                    body: Some("grpc\n".to_string()),
                },
                grpc_service: Some("grpc.health.v1.Health".to_string()),
                grpc_method: Some("Check".to_string()),
                allow_cidrs: Vec::new(),
                deny_cidrs: Vec::new(),
                requests_per_sec: None,
                burst: None,
            },
        ],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("gRPC route constraints should compile");
    let routes = &snapshot.default_vhost.routes;
    assert_eq!(routes.len(), 2);
    assert_eq!(
        routes[0].grpc_match.as_ref().and_then(|grpc| grpc.service.as_deref()),
        Some("grpc.health.v1.Health")
    );
    assert_eq!(
        routes[0].grpc_match.as_ref().and_then(|grpc| grpc.method.as_deref()),
        Some("Check")
    );
    assert!(routes[0].id.contains("grpc:service=grpc.health.v1.Health,method=Check"));
    assert!(routes[1].grpc_match.is_none());
}

#[test]
fn compile_rejects_invalid_server_access_log_format() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            proxy_protocol: None,
            default_certificate: None,
            server_names: Vec::new(),
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: Some("$trace_id $status".to_string()),
            tls: None,
        },
        upstreams: Vec::new(),
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Exact("/".to_string()),
            handler: HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("ok\n".to_string()),
            },
            grpc_service: None,

            grpc_method: None,

            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let error = compile(config).expect_err("unknown access log variables should be rejected");
    assert!(error.to_string().contains("access_log_format variable `$trace_id`"));
}

#[test]
fn compile_supports_explicit_multi_listener_configs() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: vec![
            ListenerConfig {
                name: "http".to_string(),
                proxy_protocol: None,
                default_certificate: None,
                listen: "127.0.0.1:8080".to_string(),
                trusted_proxies: Vec::new(),
                keep_alive: Some(true),
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: Some(10),
                header_read_timeout_secs: None,
                request_body_read_timeout_secs: None,
                response_write_timeout_secs: None,
                access_log_format: None,
                tls: None,
            },
            ListenerConfig {
                name: "https".to_string(),
                proxy_protocol: None,
                default_certificate: None,
                listen: "127.0.0.1:8443".to_string(),
                trusted_proxies: Vec::new(),
                keep_alive: Some(true),
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: Some(20),
                header_read_timeout_secs: None,
                request_body_read_timeout_secs: None,
                response_write_timeout_secs: None,
                access_log_format: None,
                tls: None,
            },
        ],
        server: ServerConfig {
            listen: None,
            proxy_protocol: None,
            default_certificate: None,
            server_names: vec!["example.com".to_string()],
            trusted_proxies: Vec::new(),
            keep_alive: None,
            max_headers: None,
            max_request_body_bytes: None,
            max_connections: None,
            header_read_timeout_secs: None,
            request_body_read_timeout_secs: None,
            response_write_timeout_secs: None,
            access_log_format: None,
            tls: None,
        },
        upstreams: Vec::new(),
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Exact("/".to_string()),
            handler: HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("ok\n".to_string()),
            },
            grpc_service: None,
            grpc_method: None,
            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
        }],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("explicit multi-listener config should compile");
    assert_eq!(snapshot.listeners.len(), 2);
    assert_eq!(snapshot.total_listener_count(), 2);
    assert_eq!(snapshot.listeners[0].server.listen_addr, "127.0.0.1:8080".parse().unwrap());
    assert_eq!(snapshot.listeners[0].name, "http");
    assert_eq!(snapshot.listeners[1].name, "https");
    assert_eq!(snapshot.listeners[1].server.max_connections, Some(20));
}
