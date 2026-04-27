use super::*;

mod listener_conflicts;

#[test]
fn compile_generates_deduplicated_listeners_from_vhost_listen() {
    let base_dir = temp_base_dir("rginx-vhost-listen-test-");
    fs::write(base_dir.path().join("api.crt"), b"placeholder").expect("cert should be written");
    fs::write(base_dir.path().join("api.key"), b"placeholder").expect("key should be written");

    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: server_defaults(None),
        upstreams: Vec::new(),
        locations: Vec::new(),
        servers: vec![
            VirtualHostConfig {
                listen: vec![
                    "127.0.0.1:8080".to_string(),
                    "127.0.0.1:8443 ssl http2 http3".to_string(),
                ],
                server_names: vec!["api.example.com".to_string()],
                upstreams: Vec::new(),
                locations: vec![return_location("api\n")],
                tls: Some(crate::model::VirtualHostTlsConfig {
                    cert_path: "api.crt".to_string(),
                    key_path: "api.key".to_string(),
                    additional_certificates: None,
                    ocsp_staple_path: None,
                    ocsp: None,
                }),
                http3: Some(Http3Config {
                    advertise_alt_svc: Some(true),
                    alt_svc_max_age_secs: Some(7200),
                    ..Http3Config::default()
                }),
            },
            VirtualHostConfig {
                listen: vec!["127.0.0.1:8080".to_string()],
                server_names: vec!["www.example.com".to_string()],
                upstreams: Vec::new(),
                locations: vec![return_location("www\n")],
                tls: None,
                http3: None,
            },
        ],
    };

    let snapshot =
        compile_with_base(config, base_dir.path()).expect("vhost listen config should compile");

    assert_eq!(snapshot.listeners.len(), 2);
    assert_eq!(snapshot.listeners[0].id, "vhost-listen:127.0.0.1:8080");
    assert_eq!(snapshot.listeners[0].server.listen_addr, "127.0.0.1:8080".parse().unwrap());
    assert!(!snapshot.listeners[0].tls_enabled());
    assert_eq!(snapshot.listeners[1].id, "vhost-listen:127.0.0.1:8443");
    assert!(snapshot.listeners[1].tls_enabled());
    assert_eq!(
        snapshot.listeners[1].server.default_certificate.as_deref(),
        Some("api.example.com")
    );
    let http3 = snapshot.listeners[1].http3.as_ref().expect("http3 should compile");
    assert_eq!(http3.alt_svc_max_age, Duration::from_secs(7200));
    assert_eq!(snapshot.total_listener_binding_count(), 3);
    assert_eq!(snapshot.vhosts.len(), 2);
    assert!(snapshot.default_vhost.routes.is_empty());
}

#[test]
fn compile_uses_vhost_local_upstream_before_global_upstream() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: server_defaults(None),
        upstreams: vec![upstream("backend", "http://127.0.0.1:9000")],
        locations: Vec::new(),
        servers: vec![VirtualHostConfig {
            listen: vec!["127.0.0.1:8080".to_string()],
            server_names: vec!["api.example.com".to_string()],
            upstreams: vec![upstream("backend", "http://127.0.0.1:9001")],
            locations: vec![test_location(
                MatcherConfig::Prefix("/".to_string()),
                HandlerConfig::Proxy {
                    upstream: "backend".to_string(),
                    preserve_host: None,
                    strip_prefix: None,
                    proxy_set_headers: std::collections::HashMap::new(),
                },
            )],
            tls: None,
            http3: None,
        }],
    };

    let snapshot = compile(config).expect("vhost local upstream should compile");

    assert!(snapshot.upstreams.contains_key("backend"));
    assert!(snapshot.upstreams.contains_key("servers[0]::backend"));
    match &snapshot.vhosts[0].routes[0].action {
        rginx_core::RouteAction::Proxy(proxy) => {
            assert_eq!(proxy.upstream_name, "servers[0]::backend");
            assert_eq!(proxy.upstream.name, "servers[0]::backend");
            assert_eq!(proxy.upstream.peers[0].url, "http://127.0.0.1:9001");
        }
        _ => panic!("route should proxy to local upstream"),
    }
}

#[test]
fn compile_applies_server_tls_defaults_only_to_vhost_ssl_listeners() {
    let base_dir = temp_base_dir("rginx-vhost-listen-tls-defaults-");
    fs::write(base_dir.path().join("default.crt"), b"placeholder").expect("cert should be written");
    fs::write(base_dir.path().join("default.key"), b"placeholder").expect("key should be written");
    fs::write(base_dir.path().join("api.crt"), b"placeholder").expect("cert should be written");
    fs::write(base_dir.path().join("api.key"), b"placeholder").expect("key should be written");

    let mut server = server_defaults(None);
    server.default_certificate = Some("api.example.com".to_string());
    server.tls = Some(server_tls("default.crt", "default.key"));

    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server,
        upstreams: Vec::new(),
        locations: Vec::new(),
        servers: vec![VirtualHostConfig {
            listen: vec!["127.0.0.1:8080".to_string(), "127.0.0.1:8443 ssl http2".to_string()],
            server_names: vec!["api.example.com".to_string()],
            upstreams: Vec::new(),
            locations: vec![return_location("api\n")],
            tls: Some(crate::model::VirtualHostTlsConfig {
                cert_path: "api.crt".to_string(),
                key_path: "api.key".to_string(),
                additional_certificates: None,
                ocsp_staple_path: None,
                ocsp: None,
            }),
            http3: None,
        }],
    };

    let snapshot =
        compile_with_base(config, base_dir.path()).expect("vhost listen config should compile");

    assert_eq!(snapshot.listeners.len(), 2);
    assert!(!snapshot.listeners[0].tls_enabled());
    assert!(snapshot.listeners[0].server.tls.is_none());
    assert!(snapshot.listeners[0].server.default_certificate.is_none());
    assert!(snapshot.listeners[1].tls_enabled());
    assert!(snapshot.listeners[1].server.tls.is_some());
    assert_eq!(
        snapshot.listeners[1].server.default_certificate.as_deref(),
        Some("api.example.com")
    );
}

#[test]
fn compile_uses_first_tls_vhost_as_implicit_default_certificate() {
    let base_dir = temp_base_dir("rginx-vhost-listen-implicit-default-");
    for name in ["api", "www"] {
        fs::write(base_dir.path().join(format!("{name}.crt")), b"placeholder")
            .expect("cert should be written");
        fs::write(base_dir.path().join(format!("{name}.key")), b"placeholder")
            .expect("key should be written");
    }

    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: server_defaults(None),
        upstreams: Vec::new(),
        locations: Vec::new(),
        servers: vec![
            tls_vhost("api.example.com", "api.crt", "api.key"),
            tls_vhost("www.example.com", "www.crt", "www.key"),
        ],
    };

    let snapshot =
        compile_with_base(config, base_dir.path()).expect("vhost listen config should compile");

    assert_eq!(snapshot.listeners.len(), 1);
    assert_eq!(
        snapshot.listeners[0].server.default_certificate.as_deref(),
        Some("api.example.com")
    );
}

#[test]
fn compile_preserves_ipv6_vhost_listener_ids() {
    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server: server_defaults(None),
        upstreams: Vec::new(),
        locations: Vec::new(),
        servers: vec![VirtualHostConfig {
            listen: vec!["[::1]:8080".to_string()],
            server_names: vec!["ipv6.example.com".to_string()],
            upstreams: Vec::new(),
            locations: vec![return_location("ipv6\n")],
            tls: None,
            http3: None,
        }],
    };

    let snapshot = compile(config).expect("IPv6 vhost listen config should compile");

    assert_eq!(snapshot.listeners.len(), 1);
    assert_eq!(snapshot.listeners[0].id, "vhost-listen:[::1]:8080");
    assert_eq!(snapshot.listeners[0].name, "vhost:[::1]:8080");
    assert_eq!(snapshot.listeners[0].server.listen_addr, "[::1]:8080".parse().unwrap());
}

#[test]
fn compile_supports_nezha_dashboard_native_vhost_shape() {
    let base_dir = temp_base_dir("rginx-vhost-nezha-");
    fs::write(base_dir.path().join("dashboard.crt"), b"placeholder")
        .expect("cert should be written");
    fs::write(base_dir.path().join("dashboard.key"), b"placeholder")
        .expect("key should be written");

    let mut server = server_defaults(None);
    server.trusted_proxies = vec!["0.0.0.0/0".to_string(), "::/0".to_string()];
    server.client_ip_header = Some("CF-Connecting-IP".to_string());

    let mut dashboard_grpc = upstream("dashboard_grpc", "http://127.0.0.1:8008");
    dashboard_grpc.protocol = UpstreamProtocolConfig::H2c;
    dashboard_grpc.pool_max_idle_per_host = Some(512);
    dashboard_grpc.read_timeout_secs = Some(600);
    dashboard_grpc.write_timeout_secs = Some(600);
    dashboard_grpc.http2_keep_alive_interval_secs = Some(30);

    let mut dashboard_http = upstream("dashboard_http", "http://127.0.0.1:8008");
    dashboard_http.pool_max_idle_per_host = Some(512);
    dashboard_http.read_timeout_secs = Some(3600);
    dashboard_http.write_timeout_secs = Some(3600);

    let config = Config {
        runtime: RuntimeConfig {
            shutdown_timeout_secs: 10,
            worker_threads: None,
            accept_workers: None,
        },
        listeners: Vec::new(),
        server,
        upstreams: Vec::new(),
        locations: Vec::new(),
        servers: vec![VirtualHostConfig {
            listen: vec![
                "127.0.0.1:8443 ssl http2".to_string(),
                "[::1]:8443 ssl http2".to_string(),
            ],
            server_names: vec!["dashboard.example.com".to_string()],
            upstreams: vec![dashboard_http, dashboard_grpc],
            locations: vec![
                test_location(
                    MatcherConfig::Prefix("/proto.NezhaService/".to_string()),
                    HandlerConfig::Proxy {
                        upstream: "dashboard_grpc".to_string(),
                        preserve_host: Some(true),
                        strip_prefix: None,
                        proxy_set_headers: std::collections::HashMap::from([(
                            "nz-realip".to_string(),
                            ProxyHeaderValueConfig::Dynamic(
                                ProxyHeaderDynamicValueConfig::ClientIp,
                            ),
                        )]),
                    },
                ),
                LocationConfig {
                    matcher: MatcherConfig::Regex {
                        pattern: "^/api/v1/ws/(server|terminal|file)(/.*)?$".to_string(),
                        case_insensitive: true,
                    },
                    handler: HandlerConfig::Proxy {
                        upstream: "dashboard_http".to_string(),
                        preserve_host: Some(true),
                        strip_prefix: None,
                        proxy_set_headers: std::collections::HashMap::from([
                            (
                                "nz-realip".to_string(),
                                ProxyHeaderValueConfig::Dynamic(
                                    ProxyHeaderDynamicValueConfig::ClientIp,
                                ),
                            ),
                            (
                                "origin".to_string(),
                                ProxyHeaderValueConfig::Dynamic(
                                    ProxyHeaderDynamicValueConfig::Template(
                                        "https://{host}".to_string(),
                                    ),
                                ),
                            ),
                        ]),
                    },
                    grpc_service: None,
                    grpc_method: None,
                    allow_cidrs: Vec::new(),
                    deny_cidrs: Vec::new(),
                    requests_per_sec: None,
                    burst: None,
                    allow_early_data: None,
                    request_buffering: None,
                    response_buffering: Some(RouteBufferingPolicyConfig::Off),
                    compression: Some(RouteCompressionPolicyConfig::Off),
                    compression_min_bytes: None,
                    compression_content_types: None,
                    streaming_response_idle_timeout_secs: None,
                },
                test_location(
                    MatcherConfig::Prefix("/".to_string()),
                    HandlerConfig::Proxy {
                        upstream: "dashboard_http".to_string(),
                        preserve_host: Some(true),
                        strip_prefix: None,
                        proxy_set_headers: std::collections::HashMap::from([(
                            "nz-realip".to_string(),
                            ProxyHeaderValueConfig::Dynamic(
                                ProxyHeaderDynamicValueConfig::ClientIp,
                            ),
                        )]),
                    },
                ),
            ],
            tls: Some(crate::model::VirtualHostTlsConfig {
                cert_path: "dashboard.crt".to_string(),
                key_path: "dashboard.key".to_string(),
                additional_certificates: None,
                ocsp_staple_path: None,
                ocsp: None,
            }),
            http3: None,
        }],
    };

    crate::validate::validate(&config)
        .expect("Nezha dashboard native vhost config should validate");

    let snapshot = compile_with_base(config, base_dir.path())
        .expect("Nezha dashboard native vhost config should compile");

    assert_eq!(snapshot.listeners.len(), 2);
    assert!(snapshot.listeners.iter().all(rginx_core::Listener::tls_enabled));
    assert!(snapshot.listeners.iter().all(|listener| {
        listener.server.client_ip_header.as_ref().map(|name| name.as_str())
            == Some("cf-connecting-ip")
    }));

    let vhost = &snapshot.vhosts[0];
    assert_eq!(vhost.server_names, vec!["dashboard.example.com"]);
    assert_eq!(vhost.routes.len(), 3);

    let grpc_route = vhost
        .routes
        .iter()
        .find(|route| matches!(route.matcher, rginx_core::RouteMatcher::Prefix(ref path) if path == "/proto.NezhaService/"))
        .expect("gRPC prefix route should compile");
    let rginx_core::RouteAction::Proxy(grpc_proxy) = &grpc_route.action else {
        panic!("gRPC route should proxy");
    };
    assert_eq!(grpc_proxy.upstream_name, "servers[0]::dashboard_grpc");
    assert_eq!(grpc_proxy.upstream.protocol, rginx_core::UpstreamProtocol::H2c);
    assert!(grpc_proxy.preserve_host);
    assert!(grpc_proxy.proxy_set_headers.iter().any(|(name, value)| {
        name.as_str() == "nz-realip" && matches!(value, rginx_core::ProxyHeaderValue::ClientIp)
    }));

    let ws_route = vhost
        .routes
        .iter()
        .find(|route| route.id.contains("regex:i:^/api/v1/ws/(server|terminal|file)(/.*)?$"))
        .expect("WebSocket regex route should compile");
    assert!(ws_route.matcher.matches("/API/v1/ws/server/session"));
    assert!(!ws_route.matcher.matches("/api/v1/ws/metrics"));
    assert_eq!(ws_route.response_buffering, rginx_core::RouteBufferingPolicy::Off);
    assert_eq!(ws_route.compression, rginx_core::RouteCompressionPolicy::Off);
    let rginx_core::RouteAction::Proxy(ws_proxy) = &ws_route.action else {
        panic!("WebSocket route should proxy");
    };
    assert_eq!(ws_proxy.upstream_name, "servers[0]::dashboard_http");
    assert!(ws_proxy.proxy_set_headers.iter().any(|(name, value)| {
        name.as_str() == "origin"
            && matches!(value, rginx_core::ProxyHeaderValue::Template(template) if template.as_str() == "https://{host}")
    }));

    let fallback_route = vhost
        .routes
        .iter()
        .find(|route| matches!(route.matcher, rginx_core::RouteMatcher::Prefix(ref path) if path == "/"))
        .expect("web fallback route should compile");
    let rginx_core::RouteAction::Proxy(fallback_proxy) = &fallback_route.action else {
        panic!("fallback route should proxy");
    };
    assert_eq!(fallback_proxy.upstream_name, "servers[0]::dashboard_http");
}

fn server_defaults(listen: Option<&str>) -> ServerConfig {
    ServerConfig {
        listen: listen.map(str::to_string),
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
    }
}

fn server_tls(cert_path: &str, key_path: &str) -> ServerTlsConfig {
    ServerTlsConfig {
        cert_path: cert_path.to_string(),
        key_path: key_path.to_string(),
        additional_certificates: None,
        versions: Some(vec![crate::model::TlsVersionConfig::Tls12]),
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

fn tls_vhost(server_name: &str, cert_path: &str, key_path: &str) -> VirtualHostConfig {
    VirtualHostConfig {
        listen: vec!["127.0.0.1:8443 ssl http2".to_string()],
        server_names: vec![server_name.to_string()],
        upstreams: Vec::new(),
        locations: vec![return_location("ok\n")],
        tls: Some(crate::model::VirtualHostTlsConfig {
            cert_path: cert_path.to_string(),
            key_path: key_path.to_string(),
            additional_certificates: None,
            ocsp_staple_path: None,
            ocsp: None,
        }),
        http3: None,
    }
}

fn upstream(name: &str, url: &str) -> UpstreamConfig {
    UpstreamConfig {
        name: name.to_string(),
        peers: vec![UpstreamPeerConfig { url: url.to_string(), weight: 1, backup: false }],
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
    }
}

fn return_location(body: &str) -> LocationConfig {
    test_location(
        MatcherConfig::Exact("/".to_string()),
        HandlerConfig::Return {
            status: 200,
            location: String::new(),
            body: Some(body.to_string()),
        },
    )
}
