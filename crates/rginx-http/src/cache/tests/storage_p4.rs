use std::cell::Cell;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream;
use http::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, RANGE};
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::{Body as _, Frame};
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

#[tokio::test]
async fn cache_manager_ends_downstream_stream_when_upstream_ends() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/late-end-stream")
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

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(boxed_body(LateEndStreamBody::default()))
        .expect("response should build");
    let stored = manager.store_response(context, response).await;
    assert_eq!(stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");

    let mut body = stored.into_body();
    let frame = body.frame().await.expect("frame should arrive").expect("frame should read");
    assert_eq!(frame.data_ref().unwrap().as_ref(), b"late");
    assert!(
        !body.is_end_stream(),
        "downstream body must stay open until upstream end-of-stream is observed"
    );
    assert!(body.frame().await.is_none(), "stream should finish once upstream reaches EOF");

    let response = wait_for_hit(&manager, &request, &policy).await;
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"late");
}

#[tokio::test]
async fn cache_manager_preserves_trailers_after_exact_size_hint() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/exact-trailers")
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

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(boxed_body(ExactSizeTrailersBody::default()))
        .expect("response should build");
    let stored = manager.store_response(context, response).await;
    assert_eq!(stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");

    let mut body = stored.into_body();
    let frame = body.frame().await.expect("data frame should arrive").expect("frame should read");
    assert_eq!(frame.data_ref().unwrap().as_ref(), b"late");
    assert!(!body.is_end_stream(), "exact size hint must not imply end-of-stream");

    let frame =
        body.frame().await.expect("trailers frame should arrive").expect("frame should read");
    let trailers = frame.into_trailers().expect("frame should contain trailers");
    assert_eq!(trailers.get("x-end").unwrap(), "done");
    assert!(body.frame().await.is_none(), "stream should end after trailers");

    let response = wait_for_hit(&manager, &request, &policy).await;
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"late");
}

#[derive(Default)]
struct LateEndStreamBody {
    state: Cell<u8>,
}

impl hyper::body::Body for LateEndStreamBody {
    type Data = Bytes;
    type Error = crate::handler::BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Self::Data>, Self::Error>>> {
        match self.state.get() {
            0 => {
                self.state.set(1);
                Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b"late")))))
            }
            1 => Poll::Pending,
            2 | 3 => {
                self.state.set(3);
                Poll::Ready(None)
            }
            state => panic!("unexpected poll state: {state}"),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self.state.get() {
            1 => {
                self.state.set(2);
                false
            }
            2 | 3 => true,
            _ => false,
        }
    }
}

#[derive(Default)]
struct ExactSizeTrailersBody {
    state: Cell<u8>,
}

impl hyper::body::Body for ExactSizeTrailersBody {
    type Data = Bytes;
    type Error = crate::handler::BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<Frame<Self::Data>, Self::Error>>> {
        match self.state.get() {
            0 => {
                self.state.set(1);
                Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(b"late")))))
            }
            1 => {
                self.state.set(2);
                let mut trailers = http::HeaderMap::new();
                trailers.insert("x-end", http::HeaderValue::from_static("done"));
                Poll::Ready(Some(Ok(Frame::trailers(trailers))))
            }
            2 | 3 => {
                self.state.set(3);
                Poll::Ready(None)
            }
            state => panic!("unexpected poll state: {state}"),
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(self.state.get(), 2 | 3)
    }

    fn size_hint(&self) -> hyper::body::SizeHint {
        let mut hint = hyper::body::SizeHint::default();
        hint.set_exact(4);
        hint
    }
}
