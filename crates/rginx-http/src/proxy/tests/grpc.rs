use super::*;

#[test]
fn detect_grpc_web_mode_rewrites_binary_content_type() {
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/grpc-web+proto; charset=utf-8"),
    );

    let mode = detect_grpc_web_mode(&headers)
        .expect("binary grpc-web should be supported")
        .expect("grpc-web content-type should be detected");

    assert_eq!(mode.downstream_content_type, "application/grpc-web+proto; charset=utf-8");
    assert_eq!(mode.upstream_content_type, "application/grpc+proto; charset=utf-8");
    assert_eq!(mode.encoding, GrpcWebEncoding::Binary);
}

#[test]
fn detect_grpc_web_mode_rewrites_text_content_type() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc-web-text+proto"));

    let mode = detect_grpc_web_mode(&headers)
        .expect("grpc-web-text should be supported")
        .expect("grpc-web-text content-type should be detected");

    assert_eq!(mode.downstream_content_type, "application/grpc-web-text+proto");
    assert_eq!(mode.upstream_content_type, "application/grpc+proto");
    assert_eq!(mode.encoding, GrpcWebEncoding::Text);
}

#[test]
fn parse_grpc_timeout_accepts_supported_units() {
    let cases = [
        ("1H", Duration::from_secs(60 * 60)),
        ("2M", Duration::from_secs(2 * 60)),
        ("3S", Duration::from_secs(3)),
        ("4m", Duration::from_millis(4)),
        ("5u", Duration::from_micros(5)),
        ("6n", Duration::from_nanos(6)),
        ("0n", Duration::ZERO),
    ];

    for (value, expected) in cases {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc"));
        headers.insert("grpc-timeout", HeaderValue::from_str(value).unwrap());

        let timeout = parse_grpc_timeout(&headers)
            .expect("grpc-timeout should parse")
            .expect("grpc-timeout should be present");

        assert_eq!(timeout, expected, "grpc-timeout {value} should parse correctly");
    }
}

#[test]
fn parse_grpc_timeout_rejects_invalid_values() {
    for value in ["", "1", "abc", "123456789m", "1x", "1 m"] {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc"));
        headers.insert("grpc-timeout", HeaderValue::from_str(value).unwrap());

        let error = parse_grpc_timeout(&headers).expect_err("invalid grpc-timeout should fail");
        assert!(error.contains("invalid grpc-timeout header"));
    }
}

#[test]
fn effective_upstream_request_timeout_prefers_shorter_grpc_deadline() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc"));
    headers.insert("grpc-timeout", HeaderValue::from_static("250m"));

    let timeout = effective_upstream_request_timeout(&headers, Duration::from_secs(30))
        .expect("grpc timeout should compute");

    assert_eq!(timeout, Duration::from_millis(250));
}

#[test]
fn effective_upstream_request_timeout_ignores_non_grpc_requests() {
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
    headers.insert("grpc-timeout", HeaderValue::from_static("broken"));

    let timeout = effective_upstream_request_timeout(&headers, Duration::from_secs(30))
        .expect("non-gRPC requests should ignore grpc-timeout");

    assert_eq!(timeout, Duration::from_secs(30));
}

#[test]
fn sanitize_request_headers_translates_grpc_web_requests() {
    let mut headers = HeaderMap::new();
    headers.insert(HOST, HeaderValue::from_static("client.example"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc-web+proto"));
    headers.insert("x-grpc-web", HeaderValue::from_static("1"));

    let client_address = ClientAddress {
        peer_addr: "10.2.3.4:4000".parse().unwrap(),
        client_ip: "198.51.100.9".parse().unwrap(),
        forwarded_for: "198.51.100.9".to_string(),
        source: ClientIpSource::SocketPeer,
    };
    let grpc_web_mode = GrpcWebMode {
        downstream_content_type: HeaderValue::from_static("application/grpc-web+proto"),
        upstream_content_type: HeaderValue::from_static("application/grpc+proto"),
        encoding: GrpcWebEncoding::Binary,
    };

    sanitize_request_headers(
        &mut headers,
        "127.0.0.1:9000",
        Some(HeaderValue::from_static("client.example")),
        &client_address,
        "http",
        false,
        &[],
        Some(&grpc_web_mode),
    )
    .expect("header sanitization should succeed");

    assert_eq!(headers.get(HOST).unwrap(), "127.0.0.1:9000");
    assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "application/grpc+proto");
    assert_eq!(headers.get(http::header::TE).unwrap(), "trailers");
    assert!(headers.get("x-grpc-web").is_none());
}

#[test]
fn grpc_web_text_helpers_round_trip_streamed_payloads() {
    let mut encoded = BytesMut::new();
    let first =
        encode_grpc_web_text_chunk(&mut encoded, b"hello").expect("first chunk should encode");
    let second =
        encode_grpc_web_text_chunk(&mut encoded, b" world").expect("second chunk should encode");
    let tail = flush_grpc_web_text_chunk(&mut encoded).expect("tail should flush");

    let mut decoder = BytesMut::new();
    let decoded_first = decode_grpc_web_text_chunk(&mut decoder, &first)
        .expect("first chunk should decode")
        .expect("first chunk should yield bytes");
    let decoded_second = decode_grpc_web_text_chunk(&mut decoder, &second)
        .expect("second chunk should decode")
        .expect("second chunk should yield bytes");
    let decoded_tail = decode_grpc_web_text_chunk(&mut decoder, &tail)
        .expect("tail chunk should decode")
        .expect("tail chunk should yield bytes");
    let final_flush =
        decode_grpc_web_text_final(&mut decoder).expect("final flush should decode cleanly");

    assert_eq!(decoded_first, Bytes::from_static(b"hel"));
    assert_eq!(decoded_second, Bytes::from_static(b"lo wor"));
    assert_eq!(decoded_tail, Bytes::from_static(b"ld"));
    assert!(final_flush.is_none());
}

#[test]
fn encode_grpc_web_trailers_uses_http1_header_block() {
    let mut trailers = HeaderMap::new();
    trailers.insert("grpc-status", HeaderValue::from_static("0"));
    trailers.insert("grpc-message", HeaderValue::from_static("ok"));

    let encoded = encode_grpc_web_trailers(&trailers);
    assert_eq!(encoded[0], 0x80);
    let len = u32::from_be_bytes([encoded[1], encoded[2], encoded[3], encoded[4]]) as usize;
    assert_eq!(len, encoded.len() - 5);

    let block = std::str::from_utf8(&encoded[5..]).expect("trailer block should be utf-8");
    assert!(block.contains("grpc-status: 0\r\n"));
    assert!(block.contains("grpc-message: ok\r\n"));
}

#[test]
fn extract_grpc_initial_trailers_removes_grpc_status_headers() {
    let mut headers = HeaderMap::new();
    headers.insert("grpc-status", HeaderValue::from_static("7"));
    headers.insert("grpc-message", HeaderValue::from_static("denied"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc"));

    let trailers =
        extract_grpc_initial_trailers(&mut headers).expect("grpc status headers should extract");

    assert_eq!(trailers.get("grpc-status").unwrap(), "7");
    assert_eq!(trailers.get("grpc-message").unwrap(), "denied");
    assert!(headers.get("grpc-status").is_none());
    assert!(headers.get("grpc-message").is_none());
    assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "application/grpc");
}

#[tokio::test]
async fn build_active_health_request_builds_grpc_probe_request() {
    let upstream = Upstream::new(
        "grpc-backend".to_string(),
        vec![peer("https://example.com")],
        UpstreamTls::NativeRoots,
        upstream_settings(UpstreamProtocol::Auto),
    );
    let peer = upstream.peers[0].clone();
    let check = ActiveHealthCheck {
        path: "/grpc.health.v1.Health/Check".to_string(),
        grpc_service: Some("grpc.health.v1.Health".to_string()),
        interval: Duration::from_secs(5),
        timeout: Duration::from_secs(1),
        healthy_successes_required: 1,
    };

    let request =
        build_active_health_request(&upstream, &peer, &check).expect("request should build");

    assert_eq!(request.method(), Method::POST);
    assert_eq!(request.version(), Version::HTTP_2);
    assert_eq!(
        request.uri(),
        &"https://example.com/grpc.health.v1.Health/Check".parse::<Uri>().unwrap()
    );
    assert_eq!(request.headers().get(HOST).unwrap(), "example.com");
    assert_eq!(request.headers().get(CONTENT_TYPE).unwrap(), "application/grpc");
    assert_eq!(request.headers().get(TE).unwrap(), "trailers");
    let content_length = request
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .expect("content-length should be present");

    let body = request.into_body().collect().await.expect("request body should collect").to_bytes();
    assert_eq!(content_length, body.len().to_string());
    assert_eq!(body, encode_grpc_health_check_request("grpc.health.v1.Health"));
}

#[test]
fn decode_grpc_health_check_response_reads_serving_status() {
    let encoded = grpc_health_response_body(1);

    let serving_status =
        decode_grpc_health_check_response(&encoded).expect("response should decode");

    assert_eq!(serving_status, Some(GrpcHealthServingStatus::Serving));
}

#[tokio::test]
async fn evaluate_grpc_health_probe_response_recognizes_serving_response() {
    let mut trailers = HeaderMap::new();
    trailers.insert("grpc-status", HeaderValue::from_static("0"));
    let body = StreamBody::new(stream::iter(vec![
        Ok::<_, Infallible>(Frame::data(grpc_health_response_body(1))),
        Ok(Frame::trailers(trailers)),
    ]));
    let response =
        Response::builder().status(StatusCode::OK).body(body).expect("response should build");

    let result =
        evaluate_grpc_health_probe_response(response).await.expect("response should evaluate");

    assert!(matches!(result, GrpcHealthProbeResult::Serving));
}
