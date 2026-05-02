use std::time::Duration;

use bytes::Bytes;
use futures_util::stream;
use http::header::CACHE_CONTROL;
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use tokio::time::timeout;

use crate::handler::{BoxError, boxed_body, full_body};

use super::*;

#[tokio::test]
async fn cache_manager_caches_unknown_size_response_after_stream_completion() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/streamed")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context = match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await
    {
        CacheLookup::Miss(context) => *context,
        CacheLookup::Hit(_) => panic!("empty cache should miss"),
        CacheLookup::Updating(_, _) => panic!("empty cache should not update"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    };

    let stream = stream::iter([Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(
        b"streamed body",
    )))]);
    let response = Response::builder()
        .status(StatusCode::OK)
        .body(boxed_body(StreamBody::new(stream)))
        .expect("response should build");
    let stored = manager.store_response(context, response).await;
    let body = drain_response(stored).await;
    assert_eq!(body.as_ref(), b"streamed body");

    let response = wait_for_hit(&manager, &request, &policy).await;
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"streamed body");
}

#[tokio::test]
async fn cache_manager_starts_streaming_before_upstream_body_completes() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/streaming-live")
        .header("host", "example.com")
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
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(boxed_body(StreamBody::new(stream)))
        .expect("response should build");
    let stored = timeout(Duration::from_millis(200), manager.store_response(context, response))
        .await
        .expect("streaming cache store should not wait for the whole body");
    assert_eq!(stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"chunk-1"))))
        .await
        .expect("first chunk should send");
    let mut body = stored.into_body();
    let first = body.frame().await.expect("first frame should arrive").expect("frame should read");
    assert_eq!(first.data_ref().unwrap().as_ref(), b"chunk-1");

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"chunk-2"))))
        .await
        .expect("second chunk should send");
    drop(tx);

    let second =
        body.frame().await.expect("second frame should arrive").expect("frame should read");
    assert_eq!(second.data_ref().unwrap().as_ref(), b"chunk-2");
    assert!(body.frame().await.is_none());

    let response = wait_for_hit(&manager, &request, &policy).await;
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"chunk-1chunk-2");
}

#[tokio::test]
async fn cache_manager_serves_concurrent_request_from_inflight_fill() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/shared-fill")
        .header("host", "example.com")
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
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(boxed_body(StreamBody::new(stream)))
        .expect("response should build");
    let stored = timeout(Duration::from_millis(50), manager.store_response(context, response))
        .await
        .expect("streaming cache store should not wait for the whole body");
    assert_eq!(stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"chunk-1"))))
        .await
        .expect("first chunk should send");
    let mut first_body = stored.into_body();
    let first_frame = timeout(Duration::from_millis(200), first_body.frame())
        .await
        .expect("first downstream frame should arrive")
        .expect("first downstream body should yield a frame")
        .expect("first downstream frame should read");
    assert_eq!(first_frame.data_ref().unwrap().as_ref(), b"chunk-1");

    let shared = match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await
    {
        CacheLookup::Hit(response) => response,
        CacheLookup::Miss(_) => panic!("concurrent request should reuse the in-flight fill"),
        CacheLookup::Updating(_, _) => {
            panic!("concurrent request should stream from the in-flight fill, not update")
        }
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    };
    assert_eq!(shared.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
    let mut shared_body = shared.into_body();
    let shared_frame = timeout(Duration::from_millis(200), shared_body.frame())
        .await
        .expect("shared fill frame should arrive")
        .expect("shared fill body should yield a frame")
        .expect("shared fill frame should read");
    assert_eq!(shared_frame.data_ref().unwrap().as_ref(), b"chunk-1");

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"chunk-2"))))
        .await
        .expect("second chunk should send");
    drop(tx);

    let first_remaining = first_body.collect().await.unwrap().to_bytes();
    let shared_remaining = shared_body.collect().await.unwrap().to_bytes();
    assert_eq!(
        [first_frame.data_ref().unwrap().as_ref(), first_remaining.as_ref()].concat(),
        b"chunk-1chunk-2"
    );
    assert_eq!(
        [shared_frame.data_ref().unwrap().as_ref(), shared_remaining.as_ref()].concat(),
        b"chunk-1chunk-2"
    );

    let response = wait_for_hit(&manager, &request, &policy).await;
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"chunk-1chunk-2");
}

#[tokio::test]
async fn cache_manager_continues_filling_cache_after_downstream_drop() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/streaming-drop")
        .header("host", "example.com")
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
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(boxed_body(StreamBody::new(stream)))
        .expect("response should build");
    let stored = timeout(Duration::from_millis(50), manager.store_response(context, response))
        .await
        .expect("streaming cache store should not wait for the whole body");

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"chunk-1"))))
        .await
        .expect("first chunk should send");
    let mut body = stored.into_body();
    let first = body.frame().await.expect("first frame should arrive").expect("frame should read");
    assert_eq!(first.data_ref().unwrap().as_ref(), b"chunk-1");
    drop(body);

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"chunk-2"))))
        .await
        .expect("second chunk should send");
    drop(tx);

    let response = wait_for_hit(&manager, &request, &policy).await;
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"chunk-1chunk-2");
}

#[tokio::test]
async fn inflight_fill_waits_for_unknown_vary_variant_instead_of_serving_wrong_body() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.lock_timeout = Duration::from_millis(200);

    let zh_request = Request::builder()
        .method(Method::GET)
        .uri("/vary-stream")
        .header("host", "example.com")
        .header("accept-language", "zh-CN")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context =
        match manager.lookup(CacheRequest::from_request(&zh_request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("empty cache should miss"),
        };

    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let stream =
        stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|frame| (frame, rx)) });
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .header("vary", "accept-language")
        .body(boxed_body(StreamBody::new(stream)))
        .expect("response should build");
    let owner = timeout(Duration::from_millis(50), manager.store_response(context, response))
        .await
        .expect("streaming cache store should start immediately");
    assert_eq!(owner.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
    drop(owner);

    tx.send(Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"zh"))))
        .await
        .expect("vary stream chunk should send");

    let en_request = Request::builder()
        .method(Method::GET)
        .uri("/vary-stream")
        .header("host", "example.com")
        .header("accept-language", "en-US")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let pending_lookup = manager.lookup(CacheRequest::from_request(&en_request), "https", &policy);
    assert!(
        timeout(Duration::from_millis(50), pending_lookup).await.is_err(),
        "different vary candidates must wait instead of reusing the in-flight body"
    );

    drop(tx);

    match manager.lookup(CacheRequest::from_request(&en_request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        CacheLookup::Hit(_) => panic!("different vary candidate must not hit the in-flight body"),
        CacheLookup::Updating(_, _) => panic!("empty vary candidate should not background update"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }
}
