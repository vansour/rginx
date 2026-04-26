use super::*;

#[tokio::test]
async fn grpc_error_response_builds_trailers_only_http2_error() {
    let mut headers = HeaderMap::new();
    headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/grpc"));

    let response = grpc_error_response(
        &headers,
        GrpcStatusCode::Unavailable,
        "upstream backend is unavailable",
    )
    .expect("gRPC response should be recognized");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").and_then(|value| value.to_str().ok()),
        Some("application/grpc")
    );
    assert_eq!(
        response.headers().get("grpc-status").and_then(|value| value.to_str().ok()),
        Some("14")
    );
    assert_eq!(
        response.headers().get("grpc-message").and_then(|value| value.to_str().ok()),
        Some("upstream backend is unavailable")
    );
    let body = response.into_body().collect().await.expect("body should collect").to_bytes();
    assert!(body.is_empty());
}

#[tokio::test]
async fn grpc_error_response_percent_encodes_non_ascii_messages() {
    let mut headers = HeaderMap::new();
    headers.insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/grpc"));

    let response = grpc_error_response(&headers, GrpcStatusCode::Unavailable, "café 100%")
        .expect("gRPC response should be recognized");

    assert_eq!(
        response.headers().get("grpc-message").and_then(|value| value.to_str().ok()),
        Some("caf%C3%A9 100%25")
    );
}

#[tokio::test]
async fn grpc_error_response_encodes_grpc_web_text_trailer_block() {
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/grpc-web-text+proto"),
    );

    let response = grpc_error_response(&headers, GrpcStatusCode::Unimplemented, "route not found")
        .expect("grpc-web-text response should be recognized");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get("content-type").and_then(|value| value.to_str().ok()),
        Some("application/grpc-web-text+proto")
    );
    let body = response.into_body().collect().await.expect("body should collect").to_bytes();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(body.as_ref())
        .expect("grpc-web-text body should be valid base64");
    assert_eq!(decoded[0], 0x80);
    let trailer_block = std::str::from_utf8(&decoded[5..]).expect("trailer block should be utf-8");
    assert!(trailer_block.contains("grpc-status: 12\r\n"));
    assert!(trailer_block.contains("grpc-message: route not found\r\n"));
}

#[test]
fn response_body_bytes_sent_returns_zero_for_head_requests() {
    let response = text_response(StatusCode::OK, "text/plain; charset=utf-8", "hello");
    assert_eq!(response_body_bytes_sent("HEAD", &response), Some(0));
    assert_eq!(response_body_bytes_sent("GET", &response), Some(5));
}

#[tokio::test]
async fn finalize_downstream_response_compresses_plain_text_responses() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(http::header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let compression_options = ResponseCompressionOptions::default();

    let finalized = finalize_downstream_response(
        &http::Method::GET,
        &request_headers,
        &compression_options,
        HeaderValue::from_static("req-plain"),
        text_response(
            StatusCode::OK,
            "text/plain; charset=utf-8",
            "hello compression pipeline\n".repeat(32),
        ),
        None,
        None,
        default_server_header(),
    )
    .await;

    assert!(finalized.grpc.is_none());
    assert_eq!(
        finalized
            .response
            .headers()
            .get(http::header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok()),
        Some("gzip")
    );
    assert_eq!(
        finalized.response.headers().get("x-request-id").and_then(|value| value.to_str().ok()),
        Some("req-plain")
    );
}

#[tokio::test]
async fn finalize_downstream_response_skips_compression_for_grpc_responses() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(http::header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    request_headers
        .insert(http::header::CONTENT_TYPE, HeaderValue::from_static("application/grpc"));
    let compression_options = ResponseCompressionOptions::default();

    let finalized = finalize_downstream_response(
        &http::Method::POST,
        &request_headers,
        &compression_options,
        HeaderValue::from_static("req-grpc"),
        text_response(StatusCode::OK, "application/grpc", "hello grpc pipeline\n".repeat(32)),
        grpc_request_metadata(&request_headers, "/grpc.health.v1.Health/Check"),
        None,
        default_server_header(),
    )
    .await;

    assert!(finalized.grpc.is_some());
    assert!(finalized.response.headers().get(http::header::CONTENT_ENCODING).is_none());
    assert_eq!(
        finalized.response.headers().get("x-request-id").and_then(|value| value.to_str().ok()),
        Some("req-grpc")
    );
}

#[tokio::test]
async fn finalize_downstream_response_strips_head_body_after_final_transforms() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(http::header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let compression_options = ResponseCompressionOptions::default();

    let finalized = finalize_downstream_response(
        &http::Method::HEAD,
        &request_headers,
        &compression_options,
        HeaderValue::from_static("req-head"),
        text_response(
            StatusCode::OK,
            "text/plain; charset=utf-8",
            "hello head pipeline\n".repeat(32),
        ),
        None,
        None,
        default_server_header(),
    )
    .await;

    assert!(finalized.grpc.is_none());
    assert!(finalized.response.headers().get(http::header::CONTENT_ENCODING).is_none());
    assert_eq!(finalized.body_bytes_sent, Some(0));
    let content_length = finalized
        .response
        .headers()
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .expect("HEAD response should preserve content length")
        .parse::<usize>()
        .expect("content length should parse");
    let body = finalized
        .response
        .into_body()
        .collect()
        .await
        .expect("HEAD body should collect")
        .to_bytes();
    assert!(content_length > 0);
    assert!(body.is_empty());
}

#[tokio::test]
async fn finalize_downstream_response_injects_alt_svc_when_provided() {
    let request_headers = HeaderMap::new();
    let compression_options = ResponseCompressionOptions::default();

    let finalized = finalize_downstream_response(
        &http::Method::GET,
        &request_headers,
        &compression_options,
        HeaderValue::from_static("req-alt-svc"),
        text_response(StatusCode::OK, "text/plain; charset=utf-8", "hello"),
        None,
        Some(HeaderValue::from_static("h3=\":443\"; ma=7200")),
        HeaderValue::from_static("edge-test"),
    )
    .await;

    assert_eq!(
        finalized
            .response
            .headers()
            .get(http::header::ALT_SVC)
            .and_then(|value| value.to_str().ok()),
        Some("h3=\":443\"; ma=7200")
    );
    assert_eq!(
        finalized
            .response
            .headers()
            .get(http::header::SERVER)
            .and_then(|value| value.to_str().ok()),
        Some("edge-test")
    );
    assert!(finalized.response.headers().get(http::header::DATE).is_some());
}

#[tokio::test]
async fn finalize_downstream_response_preserves_existing_date_header() {
    let request_headers = HeaderMap::new();
    let compression_options = ResponseCompressionOptions::default();
    let upstream_date = HeaderValue::from_static("Tue, 15 Nov 1994 08:12:31 GMT");
    let mut response = text_response(StatusCode::OK, "text/plain; charset=utf-8", "hello");
    response.headers_mut().insert(http::header::DATE, upstream_date.clone());

    let finalized = finalize_downstream_response(
        &http::Method::GET,
        &request_headers,
        &compression_options,
        HeaderValue::from_static("req-date"),
        response,
        None,
        None,
        default_server_header(),
    )
    .await;

    assert_eq!(finalized.response.headers().get(http::header::DATE), Some(&upstream_date));
}

#[tokio::test]
async fn finalize_downstream_response_respects_response_buffering_off() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(http::header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let compression_options = ResponseCompressionOptions {
        response_buffering: rginx_core::RouteBufferingPolicy::Off,
        ..ResponseCompressionOptions::default()
    };

    let finalized = finalize_downstream_response(
        &http::Method::GET,
        &request_headers,
        &compression_options,
        HeaderValue::from_static("req-stream"),
        text_response(
            StatusCode::OK,
            "text/plain; charset=utf-8",
            "hello response buffering\n".repeat(32),
        ),
        None,
        None,
        default_server_header(),
    )
    .await;

    assert!(finalized.response.headers().get(http::header::CONTENT_ENCODING).is_none());
}

#[tokio::test]
async fn finalize_downstream_response_force_compresses_small_responses() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(http::header::ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let compression_options = ResponseCompressionOptions {
        compression: rginx_core::RouteCompressionPolicy::Force,
        ..ResponseCompressionOptions::default()
    };

    let finalized = finalize_downstream_response(
        &http::Method::GET,
        &request_headers,
        &compression_options,
        HeaderValue::from_static("req-force"),
        text_response(StatusCode::OK, "text/plain; charset=utf-8", "a".repeat(128)),
        None,
        None,
        default_server_header(),
    )
    .await;

    assert_eq!(
        finalized
            .response
            .headers()
            .get(http::header::CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok()),
        Some("gzip")
    );
}
