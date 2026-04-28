use std::time::SystemTime;

use bytes::Bytes;
use http::header::CACHE_CONTROL;
use http::{Method, Request, Response, StatusCode};
use http_body_util::BodyExt;

use crate::handler::full_body;

use super::*;

#[tokio::test]
async fn shared_index_sync_shares_entries_between_managers() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager_a = test_manager(temp.path().to_path_buf(), 1024);
    let manager_b = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/shared")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let context =
        match manager_a.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("empty cache should miss"),
        };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("shared"))
        .expect("response should build");
    let stored = manager_a.store_response(context, response).await;
    assert_eq!(stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");

    match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"shared");
        }
        CacheLookup::Miss(_) => panic!("second manager should sync the shared index and hit"),
        CacheLookup::Updating(_, _) => panic!("fresh shared entry should not update"),
        CacheLookup::Bypass(status) => panic!("shared cache request should not bypass: {status:?}"),
    }
}

#[tokio::test]
async fn shared_index_sync_shares_admission_counts_between_managers() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager_a = test_manager(temp.path().to_path_buf(), 1024);
    let manager_b = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.min_uses = 2;
    let request = Request::builder()
        .method(Method::GET)
        .uri("/min-uses")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let context_a =
        match manager_a.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("first manager should miss"),
        };
    let response_a = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("shared min uses"))
        .expect("response should build");
    let _ = manager_a.store_response(context_a, response_a).await;

    let context_b =
        match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            _ => panic!("second manager should still miss before admission"),
        };
    let response_b = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("shared min uses"))
        .expect("response should build");
    let _ = manager_b.store_response(context_b, response_b).await;

    match manager_a.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"shared min uses");
        }
        _ => panic!("shared admission counts should allow the second manager to populate"),
    }
}

#[test]
fn shared_fill_locks_coordinate_across_zone_instances() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone_a = test_zone(temp.path().to_path_buf(), 1024);
    let zone_b = test_zone(temp.path().to_path_buf(), 1024);
    let now = unix_time_ms(SystemTime::now());

    let guard = match zone_a.fill_lock_decision("shared-key", now, Duration::from_secs(5)) {
        FillLockDecision::Acquired(guard) => guard,
        _ => panic!("first zone should acquire the shared fill lock"),
    };

    match zone_b.fill_lock_decision("shared-key", now, Duration::from_secs(5)) {
        FillLockDecision::WaitExternal { key } => assert_eq!(key, "shared-key"),
        _ => panic!("second zone should wait on the external shared fill lock"),
    }

    drop(guard);

    match zone_b.fill_lock_decision("shared-key", now, Duration::from_secs(5)) {
        FillLockDecision::Acquired(_) => {}
        _ => panic!("second zone should acquire the shared fill lock after release"),
    }
}

#[tokio::test]
async fn head_requests_can_populate_get_cache_entries_when_convert_head_is_enabled() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.methods = vec![Method::GET];
    policy.convert_head = true;

    let head_request = Request::builder()
        .method(Method::HEAD)
        .uri("/head-fill")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context =
        match manager.lookup(CacheRequest::from_request(&head_request), "https", &policy).await {
            CacheLookup::Miss(context) => *context,
            CacheLookup::Bypass(status) => {
                panic!("convert_head should allow HEAD to use GET cache methods: {status:?}")
            }
            _ => panic!("empty cache should miss for HEAD fill"),
        };
    assert_eq!(context.upstream_request_method(), Method::GET);

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("from get upstream"))
        .expect("response should build");
    let _ = manager.store_response(context, response).await;

    let get_request = Request::builder()
        .method(Method::GET)
        .uri("/head-fill")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    match manager.lookup(CacheRequest::from_request(&get_request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"from get upstream");
        }
        _ => panic!("HEAD fill should populate a GET-cacheable entry"),
    }
}
