use super::*;

#[test]
fn route_access_control_allows_when_lists_are_empty() {
    let access_control = RouteAccessControl::default();

    assert!(access_control.allows("192.0.2.10".parse::<IpAddr>().unwrap()));
}

#[test]
fn route_access_control_restricts_to_allow_list() {
    let access_control = RouteAccessControl::new(
        vec!["127.0.0.1/32".parse().unwrap(), "::1/128".parse().unwrap()],
        Vec::new(),
    );

    assert!(access_control.allows("127.0.0.1".parse::<IpAddr>().unwrap()));
    assert!(!access_control.allows("192.0.2.10".parse::<IpAddr>().unwrap()));
}

#[test]
fn route_access_control_denies_before_allowing() {
    let access_control = RouteAccessControl::new(
        vec!["10.0.0.0/8".parse().unwrap()],
        vec!["10.0.0.5/32".parse().unwrap()],
    );

    assert!(access_control.allows("10.1.2.3".parse::<IpAddr>().unwrap()));
    assert!(!access_control.allows("10.0.0.5".parse::<IpAddr>().unwrap()));
}

#[test]
fn server_matches_trusted_proxy_cidrs() {
    let server = Server {
        listen_addr: "127.0.0.1:8080".parse().unwrap(),
        server_header: default_server_header(),
        default_certificate: None,
        trusted_proxies: vec!["10.0.0.0/8".parse().unwrap(), "::1/128".parse().unwrap()],
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

    assert!(server.is_trusted_proxy("10.1.2.3".parse::<IpAddr>().unwrap()));
    assert!(server.is_trusted_proxy("::1".parse::<IpAddr>().unwrap()));
    assert!(!server.is_trusted_proxy("192.0.2.10".parse::<IpAddr>().unwrap()));
}

#[test]
fn config_snapshot_counts_routes_across_all_vhosts() {
    let server = Server {
        listen_addr: "127.0.0.1:8080".parse().unwrap(),
        server_header: default_server_header(),
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
        acme: None,
        managed_certificates: Vec::new(),
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
            routes: vec![route("/")],
            tls: None,
        },
        vhosts: vec![
            VirtualHost {
                id: "servers[0]".to_string(),
                server_names: vec!["api.example.com".to_string()],
                routes: vec![route("/users"), route("/status")],
                tls: None,
            },
            VirtualHost {
                id: "servers[1]".to_string(),
                server_names: vec!["app.example.com".to_string()],
                routes: vec![route("/")],
                tls: None,
            },
        ],
        cache_zones: HashMap::new(),
        upstreams: HashMap::new(),
    };

    assert_eq!(snapshot.total_vhost_count(), 3);
    assert_eq!(snapshot.total_route_count(), 4);
    assert_eq!(snapshot.total_listener_binding_count(), 1);
    assert!(!snapshot.http3_enabled());
}

#[test]
fn listener_transport_bindings_include_udp_http3_binding_when_configured() {
    let listener = Listener {
        id: "default".to_string(),
        name: "default".to_string(),
        server: Server {
            listen_addr: "127.0.0.1:443".parse().unwrap(),
            server_header: default_server_header(),
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
        },
        tls_termination_enabled: true,
        proxy_protocol_enabled: false,
        http3: Some(ListenerHttp3 {
            listen_addr: "127.0.0.1:443".parse().unwrap(),
            advertise_alt_svc: true,
            alt_svc_max_age: Duration::from_secs(3600),
            max_concurrent_streams: 128,
            stream_buffer_size: 64 * 1024,
            active_connection_id_limit: 2,
            retry: false,
            host_key_path: None,
            gso: false,
            early_data_enabled: false,
        }),
    };

    let bindings = listener.transport_bindings();
    assert_eq!(listener.binding_count(), 2);
    assert!(listener.http3_enabled());
    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings[0].kind, ListenerTransportKind::Tcp);
    assert_eq!(
        bindings[0].protocols,
        vec![ListenerApplicationProtocol::Http1, ListenerApplicationProtocol::Http2]
    );
    assert_eq!(bindings[1].kind, ListenerTransportKind::Udp);
    assert_eq!(bindings[1].protocols, vec![ListenerApplicationProtocol::Http3]);
    assert!(bindings[1].advertise_alt_svc);
    assert_eq!(bindings[1].alt_svc_max_age.map(|value| value.as_secs()), Some(3600));
    assert_eq!(bindings[1].http3_max_concurrent_streams, Some(128));
    assert_eq!(bindings[1].http3_stream_buffer_size, Some(64 * 1024));
    assert_eq!(bindings[1].http3_active_connection_id_limit, Some(2));
    assert_eq!(bindings[1].http3_retry, Some(false));
    assert_eq!(bindings[1].http3_host_key_path, None);
    assert_eq!(bindings[1].http3_gso, Some(false));
    assert_eq!(bindings[1].http3_early_data_enabled, Some(false));
}

#[test]
fn wildcard_server_names_require_a_subdomain() {
    let vhost = VirtualHost {
        id: "servers[0]".to_string(),
        server_names: vec!["*.example.com".to_string()],
        routes: vec![route("/")],
        tls: None,
    };

    assert!(vhost.matches_host("api.example.com"));
    assert!(vhost.matches_host("api.example.com:443"));
    assert!(!vhost.matches_host("example.com"));
}

#[test]
fn match_server_name_prefers_exact_and_rejects_root_for_wildcards() {
    assert_eq!(
        match_server_name("api.example.com", "api.example.com"),
        Some(super::super::ServerNameMatch::Exact)
    );
    assert_eq!(
        match_server_name("*.example.com", "api.example.com"),
        Some(super::super::ServerNameMatch::LeadingWildcard { suffix_len: "example.com".len() })
    );
    assert_eq!(match_server_name("*.example.com", "example.com"), None);
}

#[test]
fn dot_wildcard_server_names_match_root_and_subdomains() {
    assert_eq!(
        match_server_name(".example.com", "example.com"),
        Some(super::super::ServerNameMatch::DotWildcard { suffix_len: "example.com".len() })
    );
    assert_eq!(
        match_server_name(".example.com", "api.example.com"),
        Some(super::super::ServerNameMatch::DotWildcard { suffix_len: "example.com".len() })
    );
}

#[test]
fn trailing_wildcard_server_names_match_suffix_segments() {
    assert_eq!(
        match_server_name("mail.*", "mail.example"),
        Some(super::super::ServerNameMatch::TrailingWildcard { prefix_len: "mail".len() })
    );
    assert_eq!(
        match_server_name("mail.*", "mail.example.com"),
        Some(super::super::ServerNameMatch::TrailingWildcard { prefix_len: "mail".len() })
    );
    assert_eq!(match_server_name("mail.*", "mail"), None);
}

#[test]
fn more_specific_leading_wildcard_beats_dot_wildcard() {
    let selected = super::super::best_matching_server_name_pattern(
        [".example.com", "*.api.example.com"],
        "foo.api.example.com",
    )
    .expect("wildcard pattern should match");

    assert_eq!(selected.0, "*.api.example.com");
}

fn route(path: &str) -> Route {
    Route {
        id: format!("test|exact:{path}"),
        matcher: RouteMatcher::Exact(path.to_string()),
        grpc_match: None,
        action: RouteAction::Return(ReturnAction {
            status: StatusCode::OK,
            location: String::new(),
            body: Some("ok\n".to_string()),
        }),
        access_control: RouteAccessControl::default(),
        rate_limit: None,
        allow_early_data: false,
        request_buffering: crate::RouteBufferingPolicy::Auto,
        response_buffering: crate::RouteBufferingPolicy::Auto,
        compression: crate::RouteCompressionPolicy::Auto,
        compression_min_bytes: None,
        compression_content_types: Vec::new(),
        streaming_response_idle_timeout: None,
        cache: None,
    }
}
