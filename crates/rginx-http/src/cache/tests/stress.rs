use std::sync::Arc;

use bytes::Bytes;
use http::header::CACHE_CONTROL;
use http::{Method, Request, Response, StatusCode};
use http_body_util::BodyExt;
use tokio::task::JoinSet;

use crate::handler::full_body;

use super::*;

const STRESS_KEY_COUNT: usize = 256;
const STRESS_HIT_ROUNDS: usize = 4;
const LARGE_BODY_BYTES: usize = 256 * 1024;
const LARGE_BODY_CONCURRENCY: usize = 48;
const LARGE_BODY_ROUNDS: usize = 6;

fn stress_request(path: &str) -> Request<crate::handler::HttpBody> {
    Request::builder()
        .method(Method::GET)
        .uri(path)
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("stress request should build")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "cache stress suite; run via scripts/run-cache-stress.sh"]
async fn cache_manager_handles_large_keysets_under_parallel_fill_and_hit_load() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = Arc::new(test_manager(temp.path().to_path_buf(), 1024 * 1024));
    let policy = Arc::new(test_policy());
    let body = Bytes::from_static(b"stress-ok");

    let mut fill_tasks = JoinSet::new();
    for key_index in 0..STRESS_KEY_COUNT {
        let manager = manager.clone();
        let policy = policy.clone();
        let body = body.clone();
        fill_tasks.spawn(async move {
            let path = format!("/stress-fill/{key_index}");
            let request = stress_request(&path);
            let context = match manager
                .lookup(CacheRequest::from_request(&request), "https", &policy)
                .await
            {
                CacheLookup::Miss(context) => *context,
                CacheLookup::Hit(_) => panic!("unique stress fill key should miss"),
                CacheLookup::Updating(_, _) => {
                    panic!("unique stress fill key should not update")
                }
                CacheLookup::Bypass(status) => {
                    panic!("stress fill request should not bypass: {status:?}")
                }
            };
            let response = Response::builder()
                .status(StatusCode::OK)
                .header(CACHE_CONTROL, "max-age=60")
                .body(full_body(body))
                .expect("stress fill response should build");
            let stored = manager.store_response(context, response).await;
            assert_eq!(stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
        });
    }
    while let Some(result) = fill_tasks.join_next().await {
        result.expect("stress fill task should complete");
    }

    let zone = manager.zones.get("default").expect("zone should exist");
    assert_eq!(lock_index(&zone.index).entries.len(), STRESS_KEY_COUNT);

    for round in 0..STRESS_HIT_ROUNDS {
        let mut hit_tasks = JoinSet::new();
        for key_index in 0..STRESS_KEY_COUNT {
            let manager = manager.clone();
            let policy = policy.clone();
            let body = body.clone();
            hit_tasks.spawn(async move {
                let path = format!("/stress-fill/{key_index}");
                let request = stress_request(&path);
                match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
                    CacheLookup::Hit(response) => {
                        assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
                        let hit_body = response.into_body().collect().await.unwrap().to_bytes();
                        assert_eq!(hit_body.as_ref(), body.as_ref());
                    }
                    CacheLookup::Miss(_) => panic!("stress hit round {round} unexpectedly missed"),
                    CacheLookup::Updating(_, _) => {
                        panic!("stress hit round {round} unexpectedly updated")
                    }
                    CacheLookup::Bypass(status) => {
                        panic!("stress hit round {round} bypassed cache: {status:?}")
                    }
                }
            });
        }
        while let Some(result) = hit_tasks.join_next().await {
            result.expect("stress hit task should complete");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "cache stress suite; run via scripts/run-cache-stress.sh"]
async fn cache_manager_serves_large_cached_body_under_sustained_parallel_hits() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager =
        Arc::new(test_manager(temp.path().to_path_buf(), LARGE_BODY_BYTES.saturating_mul(2)));
    let policy = Arc::new(test_policy());
    let body = Bytes::from(vec![b'x'; LARGE_BODY_BYTES]);
    let request = stress_request("/large-body");

    let context = match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await
    {
        CacheLookup::Miss(context) => *context,
        CacheLookup::Hit(_) => panic!("large body fill should miss"),
        CacheLookup::Updating(_, _) => panic!("large body fill should not update"),
        CacheLookup::Bypass(status) => panic!("large body fill should not bypass: {status:?}"),
    };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body(body.clone()))
        .expect("large body response should build");
    let _ = manager.store_response(context, response).await;

    for _round in 0..LARGE_BODY_ROUNDS {
        let mut hit_tasks = JoinSet::new();
        for _ in 0..LARGE_BODY_CONCURRENCY {
            let manager = manager.clone();
            let policy = policy.clone();
            let expected_len = LARGE_BODY_BYTES;
            hit_tasks.spawn(async move {
                let request = stress_request("/large-body");
                match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
                    CacheLookup::Hit(response) => {
                        assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
                        let hit_body = response.into_body().collect().await.unwrap().to_bytes();
                        assert_eq!(hit_body.len(), expected_len);
                    }
                    CacheLookup::Miss(_) => panic!("large cached body unexpectedly missed"),
                    CacheLookup::Updating(_, _) => {
                        panic!("large cached body unexpectedly updated")
                    }
                    CacheLookup::Bypass(status) => {
                        panic!("large cached body bypassed cache: {status:?}")
                    }
                }
            });
        }
        while let Some(result) = hit_tasks.join_next().await {
            result.expect("large body hit task should complete");
        }
    }
}
