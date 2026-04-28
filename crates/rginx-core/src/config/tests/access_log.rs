use super::*;

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
        cache_status: Some("HIT"),
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
        cache_status: None,
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
        cache_status: None,
    });

    assert_eq!(
        rendered,
        "mtls=true subject=CN=client.example.com sans=client.example.com,api.example.com"
    );
}

#[test]
fn access_log_format_renders_cache_status_variable() {
    let format =
        AccessLogFormat::parse("cache=$cache_status").expect("access log format should parse");

    let rendered = format.render(&AccessLogValues {
        request_id: "req-3",
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
        cache_status: Some("REVALIDATED"),
    });

    assert_eq!(rendered, "cache=REVALIDATED");
}
