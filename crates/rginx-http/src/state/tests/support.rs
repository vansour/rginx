use super::*;

pub(crate) fn snapshot(listen: &str) -> ConfigSnapshot {
    let server = Server {
        listen_addr: listen.parse().unwrap(),
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
    ConfigSnapshot {
        cache_zones: HashMap::new(),
        runtime: RuntimeSettings {
            shutdown_timeout: Duration::from_secs(10),
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
        upstreams: HashMap::new(),
    }
}

pub(crate) fn snapshot_with_upstream(listen: &str) -> ConfigSnapshot {
    let mut snapshot = snapshot(listen);
    snapshot.upstreams.insert(
        "backend".to_string(),
        Arc::new(Upstream::new(
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
                active_health_check: None,
            },
        )),
    );
    snapshot
}

pub(crate) fn snapshot_with_routes(listen: &str) -> ConfigSnapshot {
    let mut snapshot = snapshot(listen);
    snapshot.default_vhost.routes = vec![Route {
        cache: None,
        id: "server/routes[0]|exact:/".to_string(),
        matcher: RouteMatcher::Exact("/".to_string()),
        grpc_match: None,
        action: RouteAction::Return(ReturnAction {
            status: StatusCode::OK,
            location: String::new(),
            body: Some("ok\n".to_string()),
        }),
        access_control: RouteAccessControl::default(),
        rate_limit: None,
        allow_early_data: false,
        request_buffering: rginx_core::RouteBufferingPolicy::Auto,
        response_buffering: rginx_core::RouteBufferingPolicy::Auto,
        compression: rginx_core::RouteCompressionPolicy::Auto,
        compression_min_bytes: None,
        compression_content_types: Vec::new(),
        streaming_response_idle_timeout: None,
    }];
    snapshot
}

pub(crate) fn snapshot_with_routes_and_upstream(listen: &str) -> ConfigSnapshot {
    let mut snapshot = snapshot_with_upstream(listen);
    snapshot.default_vhost.routes = vec![Route {
        cache: None,
        id: "server/routes[0]|exact:/".to_string(),
        matcher: RouteMatcher::Exact("/".to_string()),
        grpc_match: None,
        action: RouteAction::Return(ReturnAction {
            status: StatusCode::OK,
            location: String::new(),
            body: Some("ok\n".to_string()),
        }),
        access_control: RouteAccessControl::default(),
        rate_limit: None,
        allow_early_data: false,
        request_buffering: rginx_core::RouteBufferingPolicy::Auto,
        response_buffering: rginx_core::RouteBufferingPolicy::Auto,
        compression: rginx_core::RouteCompressionPolicy::Auto,
        compression_min_bytes: None,
        compression_content_types: Vec::new(),
        streaming_response_idle_timeout: None,
    }];
    snapshot
}

pub(crate) fn snapshot_with_cache_zone(listen: &str, path: PathBuf) -> ConfigSnapshot {
    let mut snapshot = snapshot(listen);
    snapshot.cache_zones.insert(
        "default".to_string(),
        Arc::new(rginx_core::CacheZone {
            name: "default".to_string(),
            path,
            max_size_bytes: Some(1024 * 1024),
            inactive: Duration::from_secs(60),
            default_ttl: Duration::from_secs(60),
            max_entry_bytes: 1024,
        }),
    );
    snapshot
}
