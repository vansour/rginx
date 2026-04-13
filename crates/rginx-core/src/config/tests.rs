use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use http::StatusCode;

use super::{
    AccessLogFormat, AccessLogValues, ConfigSnapshot, Listener, ListenerApplicationProtocol,
    ListenerHttp3, ListenerTransportKind, ReturnAction, Route, RouteAccessControl, RouteAction,
    RouteMatcher, RuntimeSettings, Server, VirtualHost, match_server_name,
};

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
        default_certificate: None,
        trusted_proxies: vec!["10.0.0.0/8".parse().unwrap(), "::1/128".parse().unwrap()],
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
        Some(super::ServerNameMatch::Exact)
    );
    assert_eq!(
        match_server_name("*.example.com", "api.example.com"),
        Some(super::ServerNameMatch::Wildcard { suffix_len: "example.com".len() })
    );
    assert_eq!(match_server_name("*.example.com", "example.com"), None);
}

#[test]
fn access_log_format_renders_nginx_style_variables() {
    let format = AccessLogFormat::parse(
        "reqid=$request_id remote=$remote_addr request=\"$request\" status=$status bytes=$body_bytes_sent elapsed=$request_time_ms ua=\"$http_user_agent\" referer=\"$http_referer\" grpc=$grpc_protocol svc=$grpc_service rpc=$grpc_method grpc_status=$grpc_status grpc_message=\"$grpc_message\"",
    )
    .expect("access log format should parse");

    let rendered = format.render(&AccessLogValues {
        request_id: "rginx-0000000000000042",
        remote_addr: "203.0.113.10",
        peer_addr: "10.0.0.5:45678",
        method: "GET",
        host: "app.example.com",
        path: "/hello?name=rginx",
        request: "GET /hello?name=rginx HTTP/1.1",
        status: 200,
        body_bytes_sent: Some(12),
        elapsed_ms: 7,
        client_ip_source: "x_forwarded_for",
        vhost: "servers[0]",
        route: "servers[0]/routes[0]|exact:/hello",
        scheme: "https",
        http_version: "HTTP/1.1",
        tls_version: Some("TLS1.3"),
        tls_alpn: Some("h2"),
        user_agent: Some("curl/8.7.1"),
        referer: None,
        tls_client_authenticated: true,
        tls_client_subject: Some("CN=client.example.com"),
        tls_client_issuer: Some("CN=test-ca"),
        tls_client_serial: Some("01"),
        tls_client_san_dns_names: Some("client.example.com"),
        tls_client_chain_length: Some(2),
        tls_client_chain_subjects: Some("CN=client.example.com,CN=test-ca"),
        grpc_protocol: Some("grpc-web"),
        grpc_service: Some("grpc.health.v1.Health"),
        grpc_method: Some("Check"),
        grpc_status: Some("0"),
        grpc_message: Some("ok"),
    });

    assert_eq!(
        rendered,
        "reqid=rginx-0000000000000042 remote=203.0.113.10 request=\"GET /hello?name=rginx HTTP/1.1\" status=200 bytes=12 elapsed=7 ua=\"curl/8.7.1\" referer=\"-\" grpc=grpc-web svc=grpc.health.v1.Health rpc=Check grpc_status=0 grpc_message=\"ok\""
    );
}

#[test]
fn access_log_format_rejects_unknown_variables() {
    let error = AccessLogFormat::parse("status=$status trace=$trace_id")
        .expect_err("unknown variable should fail");
    assert!(error.to_string().contains("access_log_format variable `$trace_id` is not supported"));
}

#[test]
fn access_log_format_supports_braced_variables_and_literal_dollar() {
    let format = AccessLogFormat::parse("$$ ${request_id} ${status}")
        .expect("access log format should parse");

    let rendered = format.render(&AccessLogValues {
        request_id: "req-1",
        remote_addr: "127.0.0.1",
        peer_addr: "127.0.0.1:80",
        method: "GET",
        host: "",
        path: "/",
        request: "GET / HTTP/1.1",
        status: 204,
        body_bytes_sent: None,
        elapsed_ms: 1,
        client_ip_source: "peer",
        vhost: "server",
        route: "server/routes[0]|exact:/",
        scheme: "http",
        http_version: "HTTP/1.1",
        tls_version: None,
        tls_alpn: None,
        user_agent: None,
        referer: None,
        tls_client_authenticated: false,
        tls_client_subject: None,
        tls_client_issuer: None,
        tls_client_serial: None,
        tls_client_san_dns_names: None,
        tls_client_chain_length: None,
        tls_client_chain_subjects: None,
        grpc_protocol: None,
        grpc_service: None,
        grpc_method: None,
        grpc_status: None,
        grpc_message: None,
    });

    assert_eq!(rendered, "$ req-1 204");
}

#[test]
fn access_log_format_renders_tls_client_identity_variables() {
    let format = AccessLogFormat::parse(
        "mtls=$tls_client_authenticated subject=$tls_client_subject sans=$tls_client_san_dns_names",
    )
    .expect("access log format should parse");

    let rendered = format.render(&AccessLogValues {
        request_id: "req-2",
        remote_addr: "127.0.0.1",
        peer_addr: "127.0.0.1:80",
        method: "GET",
        host: "example.com",
        path: "/",
        request: "GET / HTTP/1.1",
        status: 200,
        body_bytes_sent: Some(2),
        elapsed_ms: 1,
        client_ip_source: "peer",
        vhost: "server",
        route: "server/routes[0]|exact:/",
        scheme: "https",
        http_version: "HTTP/1.1",
        tls_version: Some("TLS1.3"),
        tls_alpn: Some("h2"),
        user_agent: None,
        referer: None,
        tls_client_authenticated: true,
        tls_client_subject: Some("CN=client.example.com"),
        tls_client_issuer: Some("CN=test-ca"),
        tls_client_serial: Some("01"),
        tls_client_san_dns_names: Some("client.example.com,api.example.com"),
        tls_client_chain_length: Some(2),
        tls_client_chain_subjects: Some("CN=client.example.com,CN=test-ca"),
        grpc_protocol: None,
        grpc_service: None,
        grpc_method: None,
        grpc_status: None,
        grpc_message: None,
    });

    assert_eq!(
        rendered,
        "mtls=true subject=CN=client.example.com sans=client.example.com,api.example.com"
    );
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
    }
}
