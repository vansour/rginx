use bytes::Bytes;
use http::header::CACHE_CONTROL;
use http::{Method, Request, Response, StatusCode};
use http_body_util::BodyExt;
use hyper::body::{Body as _, Frame};
use std::cell::Cell;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::handler::{boxed_body, full_body};

use super::*;

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
