use std::cell::Cell;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream;
use http::header::CACHE_CONTROL;
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
    let stored = timeout(Duration::from_millis(50), manager.store_response(context, response))
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
async fn cache_manager_keeps_stream_open_until_streaming_cache_finalizes() {
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
        "downstream body must stay open until cache finalization completes"
    );
    assert!(body.frame().await.is_none(), "stream should finish after finalization");

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
