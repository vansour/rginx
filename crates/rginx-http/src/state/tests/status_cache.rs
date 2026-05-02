use bytes::Bytes;
use http::StatusCode;
use http::header::CACHE_CONTROL;
use http::{Method, Request, Response};
use tempfile::tempdir;

use crate::cache::{CacheLookup, CacheRequest};
use crate::handler::full_body;

use super::*;

#[tokio::test]
async fn status_snapshot_reports_cache_zone_stats() {
    let temp = tempdir().expect("cache temp dir should exist");
    let shared = SharedState::from_config(snapshot_with_cache_zone(
        "127.0.0.1:8080",
        temp.path().to_path_buf(),
    ))
    .expect("shared state should build");

    let active = shared.snapshot().await;
    let policy = default_route_cache_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/status-cache")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context =
        match active.cache.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            CacheLookup::Hit(_) => panic!("empty cache should miss"),
            CacheLookup::Updating(_, _) => panic!("empty cache should not update"),
            CacheLookup::Bypass(status) => {
                panic!("cacheable request should not bypass: {status:?}")
            }
        };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("cached"))
        .expect("response should build");
    let _ = http_body_util::BodyExt::collect(
        active.cache.store_response(context, response).await.into_body(),
    )
    .await
    .expect("stored response body should collect");

    let status = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let status = shared.status_snapshot().await;
            if status
                .cache
                .zones
                .first()
                .is_some_and(|zone| zone.entry_count == 1 && zone.current_size_bytes == 6)
            {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("status snapshot should reflect the committed store");
    assert_eq!(status.cache.zones.len(), 1);
    assert_eq!(status.cache.zones[0].zone_name, "default");
    assert_eq!(status.cache.zones[0].entry_count, 1);
    assert_eq!(status.cache.zones[0].current_size_bytes, 6);
}
