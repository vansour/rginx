use super::*;

pub(crate) fn grpc_web_observability_body() -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x02]);
    body.extend_from_slice(b"ok");

    let trailer_block = b"grpc-status: 0\r\ngrpc-message: ok\r\n";
    body.push(0x80);
    body.extend_from_slice(&(trailer_block.len() as u32).to_be_bytes());
    body.extend_from_slice(trailer_block);
    body
}

pub(crate) fn test_config(default_vhost: VirtualHost, vhosts: Vec<VirtualHost>) -> ConfigSnapshot {
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
    ConfigSnapshot {
        runtime: RuntimeSettings {
            shutdown_timeout: Duration::from_secs(1),
            worker_threads: None,
            accept_workers: 1,
        },
        listeners: vec![rginx_core::Listener {
            id: "default".to_string(),
            name: "default".to_string(),
            server,
            tls_termination_enabled: false,
            proxy_protocol_enabled: false,
            http3: None,
        }],
        default_vhost,
        vhosts,
        upstreams: HashMap::new(),
    }
}

pub(crate) fn test_vhost(id: &str, server_names: Vec<&str>, routes: Vec<Route>) -> VirtualHost {
    VirtualHost {
        id: id.to_string(),
        server_names: server_names.into_iter().map(str::to_string).collect(),
        routes,
        tls: None,
    }
}

pub(crate) fn test_route(id: &str, matcher: RouteMatcher) -> Route {
    Route {
        id: id.to_string(),
        matcher,
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
    }
}

pub(crate) fn host_headers(host: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_str(host).expect("host header should be valid"));
    headers
}

pub(crate) fn request_uri(path: &str) -> http::Uri {
    path.parse().expect("request URI should be valid")
}
