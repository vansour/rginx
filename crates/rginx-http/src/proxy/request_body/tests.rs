use std::convert::Infallible;

use futures_util::stream;
use http::header::TE;
use http_body_util::BodyExt;
use http_body_util::StreamBody;

use super::*;
use crate::handler::{boxed_body, full_body};
use crate::proxy::grpc_web::{GrpcWebEncoding, GrpcWebMode};

fn test_request(method: Method, headers: HeaderMap, body: HttpBody) -> Request<HttpBody> {
    let mut request = Request::builder()
        .method(method)
        .uri("http://example.com/upload")
        .body(body)
        .expect("request should build");
    *request.headers_mut() = headers;
    request
}

fn chunked_body(chunks: Vec<&'static [u8]>) -> HttpBody {
    boxed_body(StreamBody::new(stream::iter(
        chunks.into_iter().map(|chunk| Ok::<_, Infallible>(Frame::data(Bytes::from_static(chunk)))),
    )))
}

fn grpc_web_text_mode() -> GrpcWebMode {
    GrpcWebMode {
        downstream_content_type: HeaderValue::from_static("application/grpc-web-text+proto"),
        upstream_content_type: HeaderValue::from_static("application/grpc+proto"),
        encoding: GrpcWebEncoding::Text,
    }
}

#[tokio::test]
async fn request_buffering_off_keeps_small_idempotent_body_streaming() {
    let prepared = PreparedProxyRequest::from_request(
        test_request(Method::PUT, HeaderMap::new(), full_body("hello")),
        "backend",
        None,
        Duration::from_secs(1),
        64 * 1024,
        Some(1024),
        RouteBufferingPolicy::Off,
        None,
    )
    .await
    .expect("request should prepare");

    assert!(!prepared.can_failover());
    let PreparedRequestBody::Streaming(Some(mut body)) = prepared.body else {
        panic!("request_buffering=Off should keep request bodies streaming");
    };
    let frame =
        body.frame().await.expect("body should yield a frame").expect("frame should succeed");
    assert_eq!(frame.into_data().expect("frame should contain data"), Bytes::from_static(b"hello"));
}

#[tokio::test]
async fn request_buffering_auto_collects_small_idempotent_body() {
    let prepared = PreparedProxyRequest::from_request(
        test_request(Method::PUT, HeaderMap::new(), full_body("hello")),
        "backend",
        None,
        Duration::from_secs(1),
        64 * 1024,
        Some(1024),
        RouteBufferingPolicy::Auto,
        None,
    )
    .await
    .expect("request should prepare");

    assert!(prepared.can_failover());
    let PreparedRequestBody::Replayable { body, trailers } = prepared.body else {
        panic!("small idempotent requests should remain replayable in Auto mode");
    };
    assert_eq!(body, Bytes::from_static(b"hello"));
    assert!(trailers.is_none());
}

#[tokio::test]
async fn request_buffering_on_collects_non_idempotent_body() {
    let prepared = PreparedProxyRequest::from_request(
        test_request(Method::POST, HeaderMap::new(), chunked_body(vec![b"hello", b" world"])),
        "backend",
        None,
        Duration::from_secs(1),
        64 * 1024,
        Some(1024),
        RouteBufferingPolicy::On,
        None,
    )
    .await
    .expect("request should prepare");

    assert!(!prepared.can_failover());
    let PreparedRequestBody::Replayable { body, trailers } = prepared.body else {
        panic!("request_buffering=On should collect request bodies when allowed");
    };
    assert_eq!(body, Bytes::from_static(b"hello world"));
    assert!(trailers.is_none());
}

#[tokio::test]
async fn request_buffering_on_preserves_te_trailers_boundary_as_streaming() {
    let mut headers = HeaderMap::new();
    headers.insert(TE, HeaderValue::from_static("trailers"));

    let prepared = PreparedProxyRequest::from_request(
        test_request(Method::PUT, headers, full_body("hello")),
        "backend",
        None,
        Duration::from_secs(1),
        64 * 1024,
        Some(1024),
        RouteBufferingPolicy::On,
        None,
    )
    .await
    .expect("request should prepare");

    assert!(!prepared.can_failover());
    assert!(
        matches!(prepared.body, PreparedRequestBody::Streaming(Some(_))),
        "TE: trailers should stay on the streaming path"
    );
}

#[tokio::test]
async fn streaming_request_body_limit_errors_midstream() {
    let prepared = PreparedProxyRequest::from_request(
        test_request(Method::POST, HeaderMap::new(), chunked_body(vec![b"hello", b"world"])),
        "backend",
        None,
        Duration::from_secs(1),
        64 * 1024,
        Some(8),
        RouteBufferingPolicy::Off,
        None,
    )
    .await
    .expect("request should prepare");

    let PreparedRequestBody::Streaming(Some(mut body)) = prepared.body else {
        panic!("request should stay streaming");
    };

    let first = body
        .frame()
        .await
        .expect("body should yield first frame")
        .expect("first frame should succeed");
    assert_eq!(first.into_data().expect("frame should contain data"), Bytes::from_static(b"hello"));

    let error = body
        .frame()
        .await
        .expect("body should yield a terminal error")
        .expect_err("second frame should exceed the configured limit");
    assert!(error.to_string().contains("request body exceeded configured limit of 8 bytes"));
}

#[tokio::test]
async fn grpc_web_text_requests_are_validated_before_upstream_dispatch() {
    let prepared = PreparedProxyRequest::from_request(
        test_request(Method::POST, HeaderMap::new(), full_body(Bytes::from_static(b"AAAAAAA="))),
        "backend",
        None,
        Duration::from_secs(1),
        64 * 1024,
        Some(1024),
        RouteBufferingPolicy::Off,
        Some(&grpc_web_text_mode()),
    )
    .await
    .expect("valid grpc-web-text body should prepare");

    let PreparedRequestBody::Replayable { body, trailers } = prepared.body else {
        panic!("grpc-web-text request bodies should be buffered for early validation");
    };
    assert_eq!(body, Bytes::from_static(b"\0\0\0\0\0"));
    assert!(trailers.is_none());
}

#[tokio::test]
async fn invalid_grpc_web_text_request_body_fails_during_preparation() {
    let error = match PreparedProxyRequest::from_request(
        test_request(Method::POST, HeaderMap::new(), full_body(Bytes::from_static(b"%%%"))),
        "backend",
        None,
        Duration::from_secs(1),
        64 * 1024,
        Some(1024),
        RouteBufferingPolicy::Off,
        Some(&grpc_web_text_mode()),
    )
    .await
    {
        Ok(_) => panic!("invalid grpc-web-text body should fail before upstream dispatch"),
        Err(error) => error,
    };

    assert!(
        error.to_string().contains("incomplete grpc-web-text base64 body"),
        "unexpected error: {error}"
    );
}
