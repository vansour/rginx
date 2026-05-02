use std::time::Duration;

use bytes::Bytes;
use futures_util::stream;
use http::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use tokio::time::timeout;

use crate::handler::{BoxError, boxed_body, full_body};

use super::*;

#[tokio::test]
async fn sliced_range_requests_stream_trim_while_filling_cache() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.range_requests = rginx_core::CacheRangeRequestPolicy::Cache;
    policy.slice_size_bytes = Some(8);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/streaming-slice")
        .header("host", "example.com")
        .header(RANGE, "bytes=2-4")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context = match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await
    {
        CacheLookup::Miss(context) => *context,
        CacheLookup::Hit(_) => panic!("empty cache should miss"),
        CacheLookup::Updating(_, _) => panic!("empty cache should not update"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    };

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let stream =
        stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|frame| (frame, rx)) });
    let response = Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(CACHE_CONTROL, "max-age=60")
        .header(CONTENT_RANGE, "bytes 0-7/26")
        .header(CONTENT_LENGTH, "8")
        .body(boxed_body(StreamBody::new(stream)))
        .expect("response should build");
    let stored = timeout(Duration::from_millis(50), manager.store_response(context, response))
        .await
        .expect("slice trim must not wait for the whole upstream slice");
    assert_eq!(stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
    assert_eq!(stored.headers().get(CONTENT_RANGE).unwrap(), "bytes 2-4/26");
    assert_eq!(stored.headers().get(CONTENT_LENGTH).unwrap(), "3");

    let mut body = stored.into_body();
    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"ab"))))
        .await
        .expect("first slice chunk should send");
    assert!(
        timeout(Duration::from_millis(50), body.frame()).await.is_err(),
        "trimmed downstream body should wait until requested bytes arrive"
    );

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"cdef"))))
        .await
        .expect("second slice chunk should send");
    let frame = timeout(Duration::from_millis(50), body.frame())
        .await
        .expect("trimmed data frame should arrive")
        .expect("body should yield a frame")
        .expect("frame should read");
    assert_eq!(frame.data_ref().unwrap().as_ref(), b"cde");

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"gh"))))
        .await
        .expect("tail slice chunk should send");
    drop(tx);
    assert!(body.frame().await.is_none(), "trimmed stream should finish after upstream EOF");

    let response = wait_for_hit(&manager, &request, &policy).await;
    assert_eq!(response.headers().get(CONTENT_RANGE).unwrap(), "bytes 2-4/26");
    assert_eq!(response.headers().get(CONTENT_LENGTH).unwrap(), "3");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"cde");
}

#[tokio::test]
async fn sliced_range_requests_share_inflight_fill_for_subranges() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.range_requests = rginx_core::CacheRangeRequestPolicy::Cache;
    policy.slice_size_bytes = Some(8);

    let first_request = Request::builder()
        .method(Method::GET)
        .uri("/shared-slice")
        .header("host", "example.com")
        .header(RANGE, "bytes=2-4")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let first_context =
        match manager.lookup(CacheRequest::from_request(&first_request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            CacheLookup::Hit(_) => panic!("empty cache should miss"),
            CacheLookup::Updating(_, _) => panic!("empty cache should not update"),
            CacheLookup::Bypass(status) => {
                panic!("cacheable request should not bypass: {status:?}")
            }
        };

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let stream =
        stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|frame| (frame, rx)) });
    let response = Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(CACHE_CONTROL, "max-age=60")
        .header(CONTENT_RANGE, "bytes 0-7/26")
        .header(CONTENT_LENGTH, "8")
        .body(boxed_body(StreamBody::new(stream)))
        .expect("response should build");
    let stored =
        timeout(Duration::from_millis(50), manager.store_response(first_context, response))
            .await
            .expect("streaming slice fill should start immediately");
    let mut first_body = stored.into_body();

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"ab"))))
        .await
        .expect("slice prefix should send");
    assert!(
        timeout(Duration::from_millis(50), first_body.frame()).await.is_err(),
        "first trimmed subrange should wait until requested bytes arrive"
    );

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"cdef"))))
        .await
        .expect("slice middle should send");
    let first_frame = timeout(Duration::from_millis(50), first_body.frame())
        .await
        .expect("first trimmed frame should arrive")
        .expect("first trimmed body should yield a frame")
        .expect("first trimmed frame should read");
    assert_eq!(first_frame.data_ref().unwrap().as_ref(), b"cde");

    let second_request = Request::builder()
        .method(Method::GET)
        .uri("/shared-slice")
        .header("host", "example.com")
        .header(RANGE, "bytes=5-6")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let shared = match manager
        .lookup(CacheRequest::from_request(&second_request), "https", &policy)
        .await
    {
        CacheLookup::Hit(response) => response,
        CacheLookup::Miss(_) => panic!("subrange should reuse the in-flight slice fill"),
        CacheLookup::Updating(_, _) => {
            panic!("subrange should stream from the in-flight slice fill")
        }
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    };
    assert_eq!(shared.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
    assert_eq!(shared.headers().get(CONTENT_RANGE).unwrap(), "bytes 5-6/26");
    assert_eq!(shared.headers().get(CONTENT_LENGTH).unwrap(), "2");
    let shared_body = shared.into_body();

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"gh"))))
        .await
        .expect("slice tail should send");
    drop(tx);

    let shared_body = timeout(Duration::from_secs(1), shared_body.collect())
        .await
        .expect("shared slice body should complete")
        .expect("shared slice body should collect")
        .to_bytes();
    assert_eq!(shared_body.as_ref(), b"fg");
    assert!(first_body.frame().await.is_none(), "first slice body should also finish after EOF");

    let response = wait_for_hit(&manager, &second_request, &policy).await;
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"fg");
}
