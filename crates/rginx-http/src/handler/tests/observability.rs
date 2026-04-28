use super::*;

#[test]
fn render_access_log_line_uses_configured_template() {
    let format = AccessLogFormat::parse(
            "ACCESS reqid=$request_id status=$status request=\"$request\" grpc=$grpc_protocol svc=$grpc_service rpc=$grpc_method grpc_status=$grpc_status grpc_message=\"$grpc_message\" bytes=$body_bytes_sent ua=\"$http_user_agent\" source=$client_ip_source route=$route",
        )
        .expect("access log format should parse");
    let client_address = ClientAddress {
        peer_addr: "10.0.0.5:4567".parse().unwrap(),
        client_ip: "203.0.113.9".parse().unwrap(),
        forwarded_for: "203.0.113.9".to_string(),
        source: ClientIpSource::XForwardedFor,
    };
    let grpc = GrpcObservability {
        protocol: "grpc-web".to_string(),
        service: "grpc.health.v1.Health".to_string(),
        method: "Check".to_string(),
        status: Some("0".to_string()),
        message: Some("ok".to_string()),
    };

    let rendered = render_access_log_line(
        &format,
        &AccessLogContext {
            request_id: "client-log-42",
            method: "GET",
            host: "app.example.com",
            path: "/demo?x=1",
            request_version: http::Version::HTTP_11,
            user_agent: Some("curl/8.7.1"),
            referer: None,
            client_address: &client_address,
            vhost: "servers[0]",
            route: "servers[0]/routes[0]|exact:/demo",
            status: 200,
            elapsed_ms: 12,
            downstream_scheme: "https",
            tls_version: Some("TLS1.3"),
            tls_alpn: Some("h2"),
            body_bytes_sent: Some(3),
            tls_client_identity: None,
            grpc: Some(&grpc),
            cache_status: Some("HIT"),
        },
    );

    assert_eq!(
        rendered,
        "ACCESS reqid=client-log-42 status=200 request=\"GET /demo?x=1 HTTP/1.1\" grpc=grpc-web svc=grpc.health.v1.Health rpc=Check grpc_status=0 grpc_message=\"ok\" bytes=3 ua=\"curl/8.7.1\" source=x_forwarded_for route=servers[0]/routes[0]|exact:/demo"
    );
}

#[test]
fn attach_connection_metadata_inserts_tls_client_identity_extension() {
    let mut request = Request::builder().uri("http://example.com/").body(()).unwrap();
    let connection = ConnectionPeerAddrs {
        socket_peer_addr: "127.0.0.1:44321".parse().unwrap(),
        proxy_protocol_source_addr: None,
        tls_client_identity: Some(TlsClientIdentity {
            subject: Some("CN=test-client".to_string()),
            issuer: Some("CN=test-ca".to_string()),
            serial_number: Some("01".to_string()),
            san_dns_names: vec!["client.example.com".to_string()],
            chain_length: 2,
            chain_subjects: vec!["CN=test-client".to_string(), "CN=test-ca".to_string()],
        }),
        tls_version: Some("TLS1.3".to_string()),
        tls_alpn: Some("h2".to_string()),
        early_data: false,
    };

    attach_connection_metadata(&mut request, &connection);

    let identity = request
        .extensions()
        .get::<TlsClientIdentity>()
        .expect("TLS client identity should be attached");
    assert_eq!(identity.subject.as_deref(), Some("CN=test-client"));
    assert_eq!(identity.san_dns_names, vec!["client.example.com"]);
}

#[test]
fn grpc_observability_extracts_request_and_response_fields() {
    let mut request_headers = HeaderMap::new();
    request_headers
        .insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/grpc-web+proto"));

    let mut response = text_response(StatusCode::OK, "application/grpc-web+proto", "ok\n");
    response.headers_mut().insert("grpc-status", HeaderValue::from_static("0"));
    response.headers_mut().insert("grpc-message", HeaderValue::from_static("ok"));

    let grpc = grpc_observability(
        grpc_request_metadata(&request_headers, "/grpc.health.v1.Health/Check"),
        response.headers(),
    )
    .expect("grpc metadata should be detected");

    assert_eq!(grpc.protocol, "grpc-web");
    assert_eq!(grpc.service, "grpc.health.v1.Health");
    assert_eq!(grpc.method, "Check");
    assert_eq!(grpc.status.as_deref(), Some("0"));
    assert_eq!(grpc.message.as_deref(), Some("ok"));
}

#[test]
fn grpc_observability_prefers_http_trailers_over_headers() {
    let mut grpc = GrpcObservability {
        protocol: "grpc".to_string(),
        service: "grpc.health.v1.Health".to_string(),
        method: "Check".to_string(),
        status: Some("0".to_string()),
        message: Some("ok".to_string()),
    };
    let mut trailers = HeaderMap::new();
    trailers.insert("grpc-status", HeaderValue::from_static("14"));
    trailers.insert("grpc-message", HeaderValue::from_static("unavailable"));

    grpc.update_from_headers(&trailers);

    assert_eq!(grpc.status.as_deref(), Some("14"));
    assert_eq!(grpc.message.as_deref(), Some("unavailable"));
}

#[test]
fn grpc_web_observability_parser_extracts_binary_trailers() {
    let mut parser = GrpcWebObservabilityParser::for_protocol("grpc-web")
        .expect("grpc-web parser should be created");
    let mut grpc = GrpcObservability {
        protocol: "grpc-web".to_string(),
        service: "grpc.health.v1.Health".to_string(),
        method: "Check".to_string(),
        status: None,
        message: None,
    };
    let body = grpc_web_observability_body();

    parser.observe_chunk(&body[..7], &mut grpc);
    parser.observe_chunk(&body[7..], &mut grpc);
    parser.finish(&mut grpc);

    assert_eq!(grpc.status.as_deref(), Some("0"));
    assert_eq!(grpc.message.as_deref(), Some("ok"));
}

#[test]
fn grpc_web_observability_parser_extracts_text_trailers() {
    let mut parser = GrpcWebObservabilityParser::for_protocol("grpc-web-text")
        .expect("grpc-web-text parser should be created");
    let mut grpc = GrpcObservability {
        protocol: "grpc-web-text".to_string(),
        service: "grpc.health.v1.Health".to_string(),
        method: "Check".to_string(),
        status: None,
        message: None,
    };
    let encoded = base64::engine::general_purpose::STANDARD.encode(grpc_web_observability_body());

    parser.observe_chunk(&encoded.as_bytes()[..9], &mut grpc);
    parser.observe_chunk(&encoded.as_bytes()[9..], &mut grpc);
    parser.finish(&mut grpc);

    assert_eq!(grpc.status.as_deref(), Some("0"));
    assert_eq!(grpc.message.as_deref(), Some("ok"));
}

#[test]
fn grpc_web_text_observability_decoder_handles_chunked_base64() {
    let mut carryover = BytesMut::new();
    carryover.extend_from_slice(b"RA==");
    let tail = decode_grpc_web_text_observability_final(&mut carryover)
        .expect("tail should decode")
        .expect("tail should yield bytes");

    assert_eq!(tail, bytes::Bytes::from_static(b"D"));
}
