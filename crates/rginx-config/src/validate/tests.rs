use crate::model::{
    Config, HandlerConfig, Http3Config, ListenerConfig, LocationConfig, MatcherConfig,
    RouteBufferingPolicyConfig, RouteCompressionPolicyConfig, RuntimeConfig, ServerConfig,
    ServerTlsConfig, TlsCipherSuiteConfig, TlsKeyExchangeGroupConfig, UpstreamConfig,
    UpstreamLoadBalanceConfig, UpstreamPeerConfig, UpstreamProtocolConfig, VirtualHostConfig,
    VirtualHostTlsConfig,
};

use super::{DEFAULT_GRPC_HEALTH_CHECK_PATH, validate};

mod listeners;
mod route;
mod runtime;
mod server_http3;
mod server_limits;
mod server_tls;
mod upstream_core;
mod upstream_health;
mod upstream_tls;
mod vhosts;

fn base_config() -> Config {
    Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: ServerConfig {
            listen: Some("127.0.0.1:8080".to_string()),
            server_header: None,
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
            http3: None,
        },
        upstreams: vec![UpstreamConfig {
            name: "backend".to_string(),
            peers: vec![UpstreamPeerConfig {
                url: "http://127.0.0.1:9000".to_string(),
                weight: 1,
                backup: false,
            }],
            tls: None,
            dns: None,
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
            allow_early_data: None,
            request_buffering: None,
            response_buffering: None,
            compression: None,
            compression_min_bytes: None,
            compression_content_types: None,
            streaming_response_idle_timeout_secs: None,
        }],
        servers: Vec::new(),
    }
}

fn valid_server_tls() -> ServerTlsConfig {
    ServerTlsConfig {
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
    }
}

fn sample_vhost(server_names: Vec<&str>) -> VirtualHostConfig {
    VirtualHostConfig {
        server_names: server_names.into_iter().map(str::to_string).collect(),
        locations: vec![LocationConfig {
            matcher: MatcherConfig::Exact("/".to_string()),
            handler: HandlerConfig::Return {
                status: 200,
                location: String::new(),
                body: Some("vhost\n".to_string()),
            },
            grpc_service: None,
            grpc_method: None,
            allow_cidrs: Vec::new(),
            deny_cidrs: Vec::new(),
            requests_per_sec: None,
            burst: None,
            allow_early_data: None,
            request_buffering: None,
            response_buffering: None,
            compression: None,
            compression_min_bytes: None,
            compression_content_types: None,
            streaming_response_idle_timeout_secs: None,
        }],
        tls: None,
    }
}
