use bytes::Bytes;
use http::header::{CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use http::{Method, Request, Response, StatusCode};
use http_body_util::BodyExt;

use crate::handler::full_body;

use super::*;

#[tokio::test]
async fn sliced_range_requests_rewrite_upstream_range_and_trim_hits() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.range_requests = rginx_core::CacheRangeRequestPolicy::Cache;
    policy.slice_size_bytes = Some(8);

    let first_request = Request::builder()
        .method(Method::GET)
        .uri("/slice")
        .header("host", "example.com")
        .header(RANGE, "bytes=2-4")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let first_context =
        match manager.lookup(CacheRequest::from_request(&first_request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("slice request should miss before storing"),
        };

    let mut upstream_headers = first_request.headers().clone();
    first_context.apply_upstream_request_headers(&mut upstream_headers);
    assert_eq!(upstream_headers.get(RANGE).unwrap(), "bytes=0-7");

    let upstream_response = Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(CONTENT_RANGE, "bytes 0-7/26")
        .header(CONTENT_LENGTH, "8")
        .body(full_body("abcdefgh"))
        .expect("response should build");
    let first_stored = manager.store_response(first_context, upstream_response).await;
    assert_eq!(first_stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
    assert_eq!(first_stored.headers().get(CONTENT_RANGE).unwrap(), "bytes 2-4/26");
    assert_eq!(first_stored.headers().get(CONTENT_LENGTH).unwrap(), "3");
    let first_body = first_stored.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(first_body.as_ref(), b"cde");

    let second_request = Request::builder()
        .method(Method::GET)
        .uri("/slice")
        .header("host", "example.com")
        .header(RANGE, "bytes=5-6")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    match manager.lookup(CacheRequest::from_request(&second_request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
            assert_eq!(response.headers().get(CONTENT_RANGE).unwrap(), "bytes 5-6/26");
            assert_eq!(response.headers().get(CONTENT_LENGTH).unwrap(), "2");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"fg");
        }
        _ => panic!("subrange within the same slice should hit the cached slice"),
    }
}

#[tokio::test]
async fn sliced_range_requests_fallback_to_passthrough_when_origin_ignores_range() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.range_requests = rginx_core::CacheRangeRequestPolicy::Cache;
    policy.slice_size_bytes = Some(8);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/slice")
        .header("host", "example.com")
        .header(RANGE, "bytes=2-4")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context = match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await
    {
        CacheLookup::Miss(context) => *context,
        _ => panic!("slice request should miss before storing"),
    };

    let upstream_response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_LENGTH, "8")
        .body(full_body("abcdefgh"))
        .expect("response should build");
    let downstream = manager.store_response(context, upstream_response).await;
    assert_eq!(downstream.status(), StatusCode::OK);
    assert_eq!(downstream.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
    assert!(downstream.headers().get(CONTENT_RANGE).is_none());
    let body = downstream.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"abcdefgh");

    assert!(matches!(
        manager.lookup(CacheRequest::from_request(&request), "https", &policy).await,
        CacheLookup::Miss(_)
    ));
}
