use std::cell::Cell;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime};

use bytes::Bytes;
use futures_util::stream;
use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::{Body as _, Frame};
use tokio::time::timeout;

use crate::handler::{BoxError, boxed_body, full_body};

use super::*;

#[tokio::test]
async fn cache_manager_returns_miss_then_hit_for_cacheable_get() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/assets/app.js?v=1")
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
        .header(CONTENT_TYPE, "text/javascript")
        .body(full_body("cached body"))
        .expect("response should build");
    let stored = manager.store_response(context, response).await;
    assert_eq!(stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
    let _ = drain_response(stored).await;

    let response = wait_for_hit(&manager, &request, &policy).await;
    assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"cached body");
}

#[tokio::test]
async fn cache_manager_does_not_store_entries_over_max_entry_size() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 4);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/large")
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
        .body(full_body("too large"))
        .expect("response should build");
    let stored = manager.store_response(context, response).await;
    assert_eq!(stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        CacheLookup::Hit(_) => panic!("oversized response should not be cached"),
        CacheLookup::Updating(_, _) => panic!("oversized response should not update"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }
}

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

#[tokio::test]
async fn cache_manager_serves_oversized_unknown_size_response_without_caching_it() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 8);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/streamed-oversized")
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

    let stream = stream::iter([
        Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"chunk"))),
        Ok::<Frame<Bytes>, BoxError>(Frame::data(Bytes::from_static(b"-overflow"))),
    ]);
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(boxed_body(StreamBody::new(stream)))
        .expect("response should build");
    let stored = manager.store_response(context, response).await;
    let body = drain_response(stored).await;
    assert_eq!(body.as_ref(), b"chunk-overflow");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        CacheLookup::Hit(_) => panic!("oversized streamed response should not be cached"),
        CacheLookup::Updating(_, _) => panic!("oversized streamed response should not update"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }
}

#[tokio::test]
async fn refresh_not_modified_response_serves_body_and_evicts_uncacheable_entry() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let key = "https:example.com:/refresh";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let cached_metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        test_metadata_input(key, now.saturating_sub(2_000), now.saturating_sub(1_000), 6),
    );
    let paths = cache_paths(temp.path(), &hash);
    write_cache_entry(&paths, &cached_metadata, b"cached").await.expect("entry should be written");
    let cached_entry = test_index_entry(
        key,
        hash.clone(),
        6,
        now.saturating_sub(1_000),
        now.saturating_sub(2_000),
    );
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(key.to_string(), cached_entry.clone());
        index.current_size_bytes = 6;
    }

    let mut context = test_store_context(zone.clone(), key);
    context.cached_entry = Some(cached_entry);
    context.cached_metadata = Some(cached_metadata);
    context.cache_status = CacheStatus::Expired;

    let response = Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(CACHE_CONTROL, "no-store")
        .body(full_body(Bytes::new()))
        .expect("304 response should build");
    let refreshed = refresh_not_modified_response(context, response)
        .await
        .expect("revalidation should reuse cached body");

    assert_eq!(refreshed.headers().get(CACHE_STATUS_HEADER).unwrap(), "REVALIDATED");
    assert_eq!(refreshed.headers().get(CACHE_CONTROL).unwrap(), "no-store");
    let body = refreshed.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"cached");
    assert!(!lock_index(&zone.index).entries.contains_key(key));
    assert!(!paths.metadata.exists());
    assert!(!paths.body.exists());
    assert_eq!(zone.stats.revalidated_total.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert_eq!(zone.stats.write_success_total.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[tokio::test]
async fn cache_manager_respects_no_cache_status_predicate() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.no_cache = Some(rginx_core::CachePredicate::Status(vec![StatusCode::OK]));
    let request = Request::builder()
        .method(Method::GET)
        .uri("/no-cache")
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
        .body(full_body("skip"))
        .expect("response should build");
    let stored = manager.store_response(context, response).await;
    assert_eq!(stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        CacheLookup::Hit(_) => panic!("response should not be cached"),
        CacheLookup::Updating(_, _) => panic!("response should not update"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }
}

#[tokio::test]
async fn cache_manager_partitions_variants_by_vary_header() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();

    let zh_request = Request::builder()
        .method(Method::GET)
        .uri("/vary")
        .header("host", "example.com")
        .header("accept-language", "zh-CN")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let zh_context = match manager
        .lookup(CacheRequest::from_request(&zh_request), "https", &policy)
        .await
    {
        CacheLookup::Miss(context) => *context,
        CacheLookup::Hit(_) => panic!("empty cache should miss"),
        CacheLookup::Updating(_, _) => panic!("empty cache should not update"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    };
    let zh_response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .header("vary", "accept-language")
        .body(full_body("zh"))
        .expect("response should build");
    let _ = drain_response(manager.store_response(zh_context, zh_response).await).await;

    let en_request = Request::builder()
        .method(Method::GET)
        .uri("/vary")
        .header("host", "example.com")
        .header("accept-language", "en-US")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    match manager.lookup(CacheRequest::from_request(&en_request), "https", &policy).await {
        CacheLookup::Miss(context) => {
            let en_response = Response::builder()
                .status(StatusCode::OK)
                .header(CACHE_CONTROL, "max-age=60")
                .header("vary", "accept-language")
                .body(full_body("en"))
                .expect("response should build");
            let _ = drain_response(manager.store_response(*context, en_response).await).await;
        }
        _ => panic!("different vary value should miss"),
    }

    let zh_response = wait_for_hit(&manager, &zh_request, &policy).await;
    let zh_body = zh_response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(zh_body.as_ref(), b"zh");

    let en_response = wait_for_hit(&manager, &en_request, &policy).await;
    let en_body = en_response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(en_body.as_ref(), b"en");
}

#[test]
fn load_index_from_disk_keeps_legacy_expired_entries_without_stale_windows() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let key = "https:example.com:/legacy";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    let now = unix_time_ms(SystemTime::now());
    std::fs::create_dir_all(&paths.dir).expect("cache dir should be created");
    std::fs::write(
        &paths.metadata,
        serde_json::to_vec(&serde_json::json!({
            "key": key,
            "stored_at_unix_ms": now.saturating_sub(2_000),
            "expires_at_unix_ms": now.saturating_sub(1_000),
            "must_revalidate": false,
            "body_size_bytes": 6
        }))
        .expect("legacy metadata should serialize"),
    )
    .expect("legacy metadata should be written");
    std::fs::write(&paths.body, b"cached").expect("legacy body should be written");

    let index = load_index_from_disk(zone.config.as_ref()).expect("index should load");
    let entry = index.entries.get(key).expect("legacy entry should be retained");

    assert_eq!(entry.hash, hash);
    assert_eq!(entry.body_size_bytes, 6);
    assert_eq!(index.current_size_bytes, 6);
    assert!(paths.metadata.exists());
    assert!(paths.body.exists());
}

#[test]
fn load_index_from_disk_removes_entries_with_invalid_vary_metadata() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let key = "https:example.com:/invalid-vary";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    let now = unix_time_ms(SystemTime::now());
    std::fs::create_dir_all(&paths.dir).expect("cache dir should be created");
    std::fs::write(
        &paths.metadata,
        serde_json::to_vec(&serde_json::json!({
            "key": key,
            "base_key": key,
            "vary": [
                { "name": "accept-language", "value": "zh-CN" },
                { "name": "bad header", "value": "broken" }
            ],
            "stored_at_unix_ms": now.saturating_sub(2_000),
            "expires_at_unix_ms": now.saturating_add(60_000),
            "must_revalidate": false,
            "body_size_bytes": 6
        }))
        .expect("invalid vary metadata should serialize"),
    )
    .expect("invalid vary metadata should be written");
    std::fs::write(&paths.body, b"cached").expect("body should be written");

    let index = load_index_from_disk(zone.config.as_ref()).expect("index should load");

    assert!(!index.entries.contains_key(key));
    assert_eq!(index.current_size_bytes, 0);
    assert!(!paths.metadata.exists());
    assert!(!paths.body.exists());
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
