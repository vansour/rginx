use super::*;

#[test]
fn compile_cache_policy_supports_p1_controls() {
    let base_dir = temp_base_dir("rginx-cache-compile-p1");
    let config = Config {
        acme: None,
        cache_zones: vec![CacheZoneConfig {
            name: "default".to_string(),
            path: "cache/default".to_string(),
            max_size_bytes: Some(1024 * 1024),
            inactive_secs: Some(120),
            default_ttl_secs: Some(30),
            max_entry_bytes: Some(1024),
            path_levels: Some(vec![1, 2]),
            loader_batch_entries: Some(25),
            loader_sleep_millis: Some(5),
            manager_batch_entries: Some(50),
            manager_sleep_millis: Some(7),
            inactive_cleanup_interval_secs: Some(11),
            shared_index: None,
        }],
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
            client_ip_header: None,
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
            cache: Some(CacheRouteConfig {
                zone: "default".to_string(),
                methods: Some(vec!["GET".to_string(), "HEAD".to_string()]),
                statuses: Some(vec![200]),
                ttl_secs_by_status: None,
                key: Some("{scheme}:{host}:{uri}".to_string()),
                cache_bypass: None,
                no_cache: None,
                stale_if_error_secs: None,
                grace_secs: None,
                keep_secs: None,
                pass_ttl_secs: None,
                use_stale: None,
                background_update: None,
                lock_timeout_secs: None,
                lock_age_secs: None,
                min_uses: Some(3),
                ignore_headers: Some(vec![
                    crate::model::CacheIgnoreHeaderConfig::SetCookie,
                    crate::model::CacheIgnoreHeaderConfig::Vary,
                ]),
                range_requests: Some(crate::model::CacheRangeRequestPolicyConfig::Cache),
                slice_size_bytes: None,
                convert_head: None,
            }),
            matcher: MatcherConfig::Prefix("/assets".to_string()),
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
    };

    let snapshot =
        compile_with_base(config, base_dir.path()).expect("p1 cache policy should compile");
    let zone = snapshot.cache_zones.get("default").expect("cache zone should compile");
    assert_eq!(zone.path_levels, vec![1, 2]);
    assert_eq!(zone.loader_batch_entries, 25);
    assert_eq!(zone.loader_sleep, Duration::from_millis(5));
    assert_eq!(zone.manager_batch_entries, 50);
    assert_eq!(zone.manager_sleep, Duration::from_millis(7));
    assert_eq!(zone.inactive_cleanup_interval, Duration::from_secs(11));

    let policy =
        snapshot.default_vhost.routes[0].cache.as_ref().expect("route cache policy should compile");
    assert_eq!(policy.min_uses, 3);
    assert_eq!(
        policy.ignore_headers,
        vec![rginx_core::CacheIgnoreHeader::SetCookie, rginx_core::CacheIgnoreHeader::Vary]
    );
    assert_eq!(policy.range_requests, rginx_core::CacheRangeRequestPolicy::Cache);
}
