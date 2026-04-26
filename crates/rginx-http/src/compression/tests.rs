use std::borrow::Cow;
use std::pin::Pin;
use std::task::{Context, Poll};

use brotli::Decompressor;
use bytes::Bytes;
use flate2::read::GzDecoder;
use http::header::{
    ACCEPT_ENCODING, ACCEPT_RANGES, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, VARY,
};
use http::{HeaderMap, HeaderValue, Method, Response, StatusCode};
use http_body_util::BodyExt;
use hyper::body::{Frame, SizeHint};
use rginx_core::{RouteBufferingPolicy, RouteCompressionPolicy};
use std::io::Read;

use super::{
    ContentCoding, ResponseCompressionOptions, maybe_encode_response, preferred_response_encoding,
};
use crate::handler::{BoxError, text_response};

#[derive(Debug, Default)]
struct CollectErrorBody;

impl hyper::body::Body for CollectErrorBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(Some(Err(std::io::Error::other("collect failed"))))
    }

    fn is_end_stream(&self) -> bool {
        false
    }

    fn size_hint(&self) -> SizeHint {
        let mut hint = SizeHint::new();
        hint.set_exact(512);
        hint
    }
}

#[test]
fn preferred_response_encoding_honors_quality_values() {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("br, gzip;q=0.5"));
    assert_eq!(preferred_response_encoding(&headers), Some(ContentCoding::Brotli));

    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("br;q=0.1, gzip;q=0.9"));
    assert_eq!(preferred_response_encoding(&headers), Some(ContentCoding::Gzip));

    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("br;q=0, *;q=0.5"));
    assert_eq!(preferred_response_encoding(&headers), Some(ContentCoding::Gzip));

    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip;q=0"));
    assert_eq!(preferred_response_encoding(&headers), None);

    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip;Q=0.5, br;q=0.1"));
    assert_eq!(preferred_response_encoding(&headers), Some(ContentCoding::Gzip));
}

#[tokio::test]
async fn maybe_encode_response_brotlis_text_bodies() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("br, gzip;q=0.5"));
    let options = ResponseCompressionOptions::default();

    let response = text_response(
        StatusCode::OK,
        "text/plain; charset=utf-8",
        "hello brotli world\n".repeat(32),
    );

    let response = maybe_encode_response(&Method::GET, &request_headers, &options, response).await;
    assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "br");
    assert_eq!(response.headers().get(http::header::VARY).unwrap(), "Accept-Encoding");

    let compressed =
        response.into_body().collect().await.expect("compressed body should collect").to_bytes();
    let mut decoder = Decompressor::new(compressed.as_ref(), 4096);
    let mut decoded = String::new();
    decoder.read_to_string(&mut decoded).expect("brotli payload should decode");

    assert_eq!(decoded, "hello brotli world\n".repeat(32));
}

#[tokio::test]
async fn maybe_encode_response_gzips_text_bodies() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip, br;q=0.1"));
    let options = ResponseCompressionOptions::default();

    let response =
        text_response(StatusCode::OK, "text/plain; charset=utf-8", "hello gzip world\n".repeat(32));

    let response = maybe_encode_response(&Method::GET, &request_headers, &options, response).await;
    assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "gzip");
    assert_eq!(response.headers().get(http::header::VARY).unwrap(), "Accept-Encoding");

    let compressed =
        response.into_body().collect().await.expect("compressed body should collect").to_bytes();
    let mut decoder = GzDecoder::new(compressed.as_ref());
    let mut decoded = String::new();
    decoder.read_to_string(&mut decoded).expect("gzip payload should decode");

    assert_eq!(decoded, "hello gzip world\n".repeat(32));
}

#[tokio::test]
async fn maybe_encode_response_skips_partial_content() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let options = ResponseCompressionOptions::default();

    let response = Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(http::header::CONTENT_LENGTH, "512")
        .header(http::header::CONTENT_RANGE, "bytes 0-511/2048")
        .header(ACCEPT_RANGES, "bytes")
        .body(crate::handler::full_body(Bytes::from(vec![b'a'; 512])))
        .expect("partial content response should build");

    let response = maybe_encode_response(&Method::GET, &request_headers, &options, response).await;
    assert!(response.headers().get(CONTENT_ENCODING).is_none());
    assert_eq!(response.headers().get(http::header::CONTENT_RANGE).unwrap(), "bytes 0-511/2048");
}

#[tokio::test]
async fn maybe_encode_response_skips_small_or_binary_bodies() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let options = ResponseCompressionOptions::default();

    let small = text_response(StatusCode::OK, "text/plain; charset=utf-8", "tiny");
    let small = maybe_encode_response(&Method::GET, &request_headers, &options, small).await;
    assert!(small.headers().get(CONTENT_ENCODING).is_none());

    let binary = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/grpc")
        .header(http::header::CONTENT_LENGTH, "512")
        .body(crate::handler::full_body(Bytes::from(vec![0_u8; 512])))
        .expect("binary response should build");
    let binary = maybe_encode_response(&Method::GET, &request_headers, &options, binary).await;
    assert!(binary.headers().get(CONTENT_ENCODING).is_none());
}

#[tokio::test]
async fn maybe_encode_response_merges_existing_vary_header() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let options = ResponseCompressionOptions::default();

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(CONTENT_LENGTH, "640")
        .header(VARY, "Origin")
        .body(crate::handler::full_body(Bytes::from(vec![b'a'; 640])))
        .expect("response should build");

    let response = maybe_encode_response(&Method::GET, &request_headers, &options, response).await;
    let vary = response
        .headers()
        .get(VARY)
        .and_then(|value| value.to_str().ok())
        .expect("compressed response should keep vary");

    assert!(vary.contains("Origin"), "vary should retain existing dimensions: {vary}");
    assert!(
        vary.contains("Accept-Encoding"),
        "vary should advertise compression negotiation: {vary}"
    );
}

#[tokio::test]
async fn maybe_encode_response_preserves_vary_wildcard() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let options = ResponseCompressionOptions::default();

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(CONTENT_LENGTH, "640")
        .header(VARY, "*")
        .body(crate::handler::full_body(Bytes::from(vec![b'a'; 640])))
        .expect("response should build");

    let response = maybe_encode_response(&Method::GET, &request_headers, &options, response).await;
    assert_eq!(response.headers().get(VARY).and_then(|value| value.to_str().ok()), Some("*"));
}

#[tokio::test]
async fn maybe_encode_response_does_not_leave_stale_content_length_on_collect_failure() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let options = ResponseCompressionOptions::default();

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(CONTENT_LENGTH, "512")
        .body(CollectErrorBody.map_err(|error| -> BoxError { Box::new(error) }).boxed_unsync())
        .expect("response should build");

    let response = maybe_encode_response(&Method::GET, &request_headers, &options, response).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let advertised_length = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());

    match response.into_body().collect().await {
        Ok(collected) => {
            let body = collected.to_bytes();
            assert_eq!(
                advertised_length.unwrap_or(body.len()),
                body.len(),
                "collect failure should not degrade into an empty body with stale content-length",
            );
        }
        Err(_) => {
            assert!(
                advertised_length.is_none(),
                "erroring fallback responses should not retain a stale content-length"
            );
        }
    }
}

#[tokio::test]
async fn maybe_encode_response_rebuilds_buffered_fallback_length_when_compression_is_not_smaller() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let options = ResponseCompressionOptions {
        response_buffering: RouteBufferingPolicy::On,
        compression: RouteCompressionPolicy::Force,
        compression_min_bytes: Some(1),
        ..ResponseCompressionOptions::default()
    };
    let body = "abcdefghijklmnopqrstuvwxyz";

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(ACCEPT_RANGES, "bytes")
        .body(crate::handler::full_body(body))
        .expect("response should build");

    let response = maybe_encode_response(&Method::GET, &request_headers, &options, response).await;
    assert!(response.headers().get(CONTENT_ENCODING).is_none());
    assert_eq!(
        response.headers().get(CONTENT_LENGTH).and_then(|value| value.to_str().ok()),
        Some("26")
    );
    assert_eq!(
        response.headers().get(ACCEPT_RANGES).and_then(|value| value.to_str().ok()),
        Some("bytes")
    );

    let collected = response.into_body().collect().await.expect("body should collect").to_bytes();
    assert_eq!(collected.as_ref(), body.as_bytes());
}

#[tokio::test]
async fn maybe_encode_response_skips_when_response_buffering_is_off() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let options = ResponseCompressionOptions {
        response_buffering: RouteBufferingPolicy::Off,
        ..ResponseCompressionOptions::default()
    };

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .header(CONTENT_LENGTH, "512")
        .body(CollectErrorBody.map_err(|error| -> BoxError { Box::new(error) }).boxed_unsync())
        .expect("response should build");

    let response = maybe_encode_response(&Method::GET, &request_headers, &options, response).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(CONTENT_ENCODING).is_none());
}

#[tokio::test]
async fn maybe_encode_response_respects_custom_content_type_allowlist() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let options = ResponseCompressionOptions {
        compression_content_types: Cow::Owned(vec!["application/json".to_string()]),
        ..ResponseCompressionOptions::default()
    };

    let response =
        text_response(StatusCode::OK, "text/plain; charset=utf-8", "hello allowlist\n".repeat(32));

    let response = maybe_encode_response(&Method::GET, &request_headers, &options, response).await;
    assert!(response.headers().get(CONTENT_ENCODING).is_none());
}

#[tokio::test]
async fn maybe_encode_response_respects_custom_min_bytes() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let options = ResponseCompressionOptions {
        compression_min_bytes: Some(1024),
        ..ResponseCompressionOptions::default()
    };

    let response = text_response(StatusCode::OK, "text/plain; charset=utf-8", "a".repeat(640));

    let response = maybe_encode_response(&Method::GET, &request_headers, &options, response).await;
    assert!(response.headers().get(CONTENT_ENCODING).is_none());
}

#[tokio::test]
async fn maybe_encode_response_force_allows_small_responses() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let options = ResponseCompressionOptions {
        compression: RouteCompressionPolicy::Force,
        ..ResponseCompressionOptions::default()
    };

    let response = text_response(StatusCode::OK, "text/plain; charset=utf-8", "a".repeat(128));

    let response = maybe_encode_response(&Method::GET, &request_headers, &options, response).await;
    assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "gzip");
}

#[tokio::test]
async fn maybe_encode_response_response_buffering_on_can_use_exact_size_hint() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let options = ResponseCompressionOptions {
        response_buffering: RouteBufferingPolicy::On,
        ..ResponseCompressionOptions::default()
    };

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(crate::handler::full_body(Bytes::from(vec![b'a'; 512])))
        .expect("response should build");

    let response = maybe_encode_response(&Method::GET, &request_headers, &options, response).await;
    assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "gzip");
}

#[tokio::test]
async fn maybe_encode_response_auto_skips_when_content_length_is_unknown() {
    let mut request_headers = HeaderMap::new();
    request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
    let options = ResponseCompressionOptions::default();

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(crate::handler::full_body(Bytes::from(vec![b'a'; 512])))
        .expect("response should build");

    let response = maybe_encode_response(&Method::GET, &request_headers, &options, response).await;
    assert!(response.headers().get(CONTENT_ENCODING).is_none());
}
