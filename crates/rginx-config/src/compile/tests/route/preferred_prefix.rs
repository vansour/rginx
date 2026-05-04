use super::*;

#[test]
fn compile_attaches_preferred_prefix_matcher_without_reordering_routes() {
    let config = Config {
        acme: None,
        cache_zones: Vec::new(),
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
        upstreams: Vec::new(),
        locations: vec![
            test_location(
                MatcherConfig::PreferredPrefix("/assets".to_string()),
                HandlerConfig::Return {
                    status: 200,
                    location: String::new(),
                    body: Some("assets\n".to_string()),
                },
            ),
            test_location(
                MatcherConfig::Regex {
                    pattern: "^/assets/.*$".to_string(),
                    case_insensitive: false,
                },
                HandlerConfig::Return {
                    status: 200,
                    location: String::new(),
                    body: Some("regex\n".to_string()),
                },
            ),
        ],
        servers: Vec::new(),
    };

    let snapshot = compile(config).expect("preferred prefix route should compile");
    assert_eq!(snapshot.default_vhost.routes[0].id, "server/routes[0]|preferred_prefix:/assets");
    assert_eq!(snapshot.default_vhost.routes[1].id, "server/routes[1]|regex:^/assets/.*$");
}
