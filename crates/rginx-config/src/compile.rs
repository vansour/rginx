use std::path::{Path, PathBuf};

use crate::model::Config;
use rginx_core::{ConfigSnapshot, Result, VirtualHost};

use crate::validate::validate;

mod route;
mod runtime;
mod server;
mod upstream;
mod vhost;

const DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS: u64 = 30;
const DEFAULT_UPSTREAM_CONNECT_TIMEOUT_SECS: u64 = DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS;
const DEFAULT_UPSTREAM_WRITE_TIMEOUT_SECS: u64 = DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS;
const DEFAULT_UPSTREAM_IDLE_TIMEOUT_SECS: u64 = DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS;
const DEFAULT_UPSTREAM_POOL_IDLE_TIMEOUT_SECS: u64 = 90;
const DEFAULT_UPSTREAM_POOL_MAX_IDLE_PER_HOST: usize = usize::MAX;
const DEFAULT_UPSTREAM_HTTP2_KEEP_ALIVE_TIMEOUT_SECS: u64 = 20;
const DEFAULT_MAX_REPLAYABLE_REQUEST_BODY_BYTES: u64 = 64 * 1024;
const DEFAULT_UNHEALTHY_AFTER_FAILURES: u32 = 2;
const DEFAULT_UNHEALTHY_COOLDOWN_SECS: u64 = 10;
const DEFAULT_HEALTH_CHECK_INTERVAL_SECS: u64 = 5;
const DEFAULT_HEALTH_CHECK_TIMEOUT_SECS: u64 = 2;
const DEFAULT_HEALTHY_SUCCESSES_REQUIRED: u32 = 2;
const DEFAULT_VHOST_ID: &str = "server";
const DEFAULT_GRPC_HEALTH_CHECK_PATH: &str = "/grpc.health.v1.Health/Check";

pub fn compile(raw: Config) -> Result<ConfigSnapshot> {
    compile_with_base(raw, Path::new("."))
}

pub fn compile_with_base(raw: Config, base_dir: impl AsRef<Path>) -> Result<ConfigSnapshot> {
    validate(&raw)?;
    let base_dir = base_dir.as_ref();

    let Config {
        runtime,
        listeners: raw_listeners,
        server,
        upstreams: raw_upstreams,
        locations,
        servers: raw_servers,
    } = raw;
    let runtime = runtime::compile_runtime_settings(runtime)?;
    let any_vhost_tls = raw_servers.iter().any(|vhost| vhost.tls.is_some());
    let (listeners, primary_server, default_server_names, default_server_tls) =
        if raw_listeners.is_empty() {
            let compiled_server = server::compile_legacy_server(server, base_dir, any_vhost_tls)?;
            (
                vec![compiled_server.listener.clone()],
                compiled_server.listener.server.clone(),
                compiled_server.server_names,
                compiled_server.listener.server.tls.clone(),
            )
        } else {
            let default_server_names = server.server_names;
            let listeners = server::compile_listeners(raw_listeners, base_dir)?;
            let primary_server = listeners
                .first()
                .expect("at least one explicit listener should be compiled")
                .server
                .clone();
            (listeners, primary_server, default_server_names, None)
        };
    let upstreams = upstream::compile_upstreams(raw_upstreams, base_dir)?;

    let default_vhost = VirtualHost {
        id: DEFAULT_VHOST_ID.to_string(),
        server_names: default_server_names,
        routes: route::compile_routes(locations, &upstreams, DEFAULT_VHOST_ID)?,
        tls: default_server_tls,
    };

    let vhosts = raw_servers
        .into_iter()
        .enumerate()
        .map(|(index, vhost_config)| {
            vhost::compile_virtual_host(
                format!("servers[{index}]"),
                vhost_config,
                &upstreams,
                base_dir,
            )
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(ConfigSnapshot {
        runtime,
        server: primary_server,
        listeners,
        default_vhost,
        vhosts,
        upstreams,
    })
}

pub(super) fn resolve_path(base_dir: &Path, path: String) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() { path } else { base_dir.join(path) }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

    use crate::model::{
        Config, HandlerConfig, ListenerConfig, LocationConfig, MatcherConfig, RuntimeConfig,
        ServerConfig, ServerTlsConfig, UpstreamConfig, UpstreamLoadBalanceConfig,
        UpstreamPeerConfig, UpstreamProtocolConfig, UpstreamTlsConfig, VirtualHostConfig,
    };

    use super::{
        DEFAULT_HEALTH_CHECK_INTERVAL_SECS, DEFAULT_HEALTH_CHECK_TIMEOUT_SECS,
        DEFAULT_HEALTHY_SUCCESSES_REQUIRED, DEFAULT_MAX_REPLAYABLE_REQUEST_BODY_BYTES,
        DEFAULT_UNHEALTHY_AFTER_FAILURES, DEFAULT_UNHEALTHY_COOLDOWN_SECS,
        DEFAULT_UPSTREAM_HTTP2_KEEP_ALIVE_TIMEOUT_SECS, DEFAULT_UPSTREAM_POOL_IDLE_TIMEOUT_SECS,
        DEFAULT_UPSTREAM_REQUEST_TIMEOUT_SECS, compile, compile_with_base,
    };

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
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let base_dir = std::env::temp_dir().join(format!("rginx-config-test-{unique}"));
        fs::create_dir_all(&base_dir).expect("temp base dir should be created");
        let ca_path = base_dir.join("dev-ca.pem");
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
                tls: Some(UpstreamTlsConfig::CustomCa { ca_cert_path: "dev-ca.pem".to_string() }),
                protocol: UpstreamProtocolConfig::Http2,
                load_balance: UpstreamLoadBalanceConfig::RoundRobin,
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
            compile_with_base(config, &base_dir).expect("custom CA config should compile");
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

        fs::remove_file(&ca_path).expect("temp CA file should be removed");
        fs::remove_dir(&base_dir).expect("temp base dir should be removed");
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
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let base_dir = std::env::temp_dir().join(format!("rginx-server-tls-config-test-{unique}"));
        fs::create_dir_all(&base_dir).expect("temp base dir should be created");
        let cert_path = base_dir.join("server.crt");
        let key_path = base_dir.join("server.key");
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

        let snapshot = compile_with_base(config, &base_dir).expect("server TLS should compile");
        let tls = snapshot.server.tls.expect("compiled server TLS should exist");
        assert_eq!(tls.cert_path, cert_path);
        assert_eq!(tls.key_path, key_path);

        fs::remove_file(cert_path).expect("temp cert file should be removed");
        fs::remove_file(key_path).expect("temp key file should be removed");
        fs::remove_dir(base_dir).expect("temp base dir should be removed");
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
        assert_eq!(snapshot.server.trusted_proxies.len(), 2);
        assert!(snapshot.server.is_trusted_proxy("10.1.2.3".parse().unwrap()));
        assert!(snapshot.server.is_trusted_proxy("127.0.0.1".parse().unwrap()));
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
        assert!(!snapshot.server.keep_alive);
        assert_eq!(snapshot.server.max_headers, Some(32));
        assert_eq!(snapshot.server.max_request_body_bytes, Some(1024));
        assert_eq!(snapshot.server.max_connections, Some(256));
        assert_eq!(snapshot.server.header_read_timeout, Some(Duration::from_secs(3)));
        assert_eq!(snapshot.server.request_body_read_timeout, Some(Duration::from_secs(4)));
        assert_eq!(snapshot.server.response_write_timeout, Some(Duration::from_secs(5)));
        let access_log_format =
            snapshot.server.access_log_format.as_ref().expect("access log format should compile");
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
        assert_eq!(snapshot.server.listen_addr, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(snapshot.listeners[0].name, "http");
        assert_eq!(snapshot.listeners[1].name, "https");
        assert_eq!(snapshot.listeners[1].server.max_connections, Some(20));
    }
}
