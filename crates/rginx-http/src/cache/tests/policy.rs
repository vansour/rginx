use std::time::Duration;

use http::StatusCode;
use http::header::{CACHE_CONTROL, EXPIRES, HeaderMap, HeaderValue, PRAGMA};

use super::*;

#[test]
fn response_ttl_respects_explicit_zero_or_expired_freshness() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let context = test_store_context(zone, "/ttl");

    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=0"));
    assert_eq!(response_ttl(&context, http::StatusCode::OK, &headers), Duration::ZERO);

    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let context = test_store_context(zone, "/ttl");
    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=60, s-maxage=10"));
    assert_eq!(response_ttl(&context, http::StatusCode::OK, &headers), Duration::from_secs(10));

    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let context = test_store_context(zone, "/ttl");
    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=invalid, s-maxage=120"));
    assert_eq!(response_ttl(&context, http::StatusCode::OK, &headers), Duration::from_secs(120));

    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let context = test_store_context(zone, "/ttl");
    let mut headers = HeaderMap::new();
    headers.insert(EXPIRES, HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"));
    assert_eq!(response_ttl(&context, http::StatusCode::OK, &headers), Duration::ZERO);
}

#[test]
fn request_requires_revalidation_honors_pragma_no_cache() {
    let mut headers = HeaderMap::new();
    headers.insert(PRAGMA, HeaderValue::from_static("no-cache"));

    assert!(request_requires_revalidation(&headers));
}

#[test]
fn response_ttl_prefers_x_accel_expires_and_status_rule() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let mut context = test_store_context(zone, "/ttl");
    context.policy.ttl_by_status = vec![rginx_core::CacheStatusTtlRule {
        statuses: vec![StatusCode::NOT_FOUND],
        ttl: Duration::from_secs(9),
    }];

    let mut headers = HeaderMap::new();
    assert_eq!(response_ttl(&context, StatusCode::NOT_FOUND, &headers), Duration::from_secs(9));

    headers.insert("x-accel-expires", HeaderValue::from_static("30"));
    assert_eq!(response_ttl(&context, StatusCode::NOT_FOUND, &headers), Duration::from_secs(30));

    headers.insert("x-accel-expires", HeaderValue::from_static("0"));
    assert_eq!(response_ttl(&context, StatusCode::NOT_FOUND, &headers), Duration::ZERO);
}
