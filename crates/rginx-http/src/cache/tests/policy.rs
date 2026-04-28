use std::time::Duration;

use http::header::{CACHE_CONTROL, EXPIRES, HeaderMap, HeaderValue};

use super::*;

#[test]
fn response_ttl_respects_explicit_zero_or_expired_freshness() {
    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=0"));
    assert_eq!(response_ttl(&headers, Duration::from_secs(60)), Duration::ZERO);

    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=60, s-maxage=10"));
    assert_eq!(response_ttl(&headers, Duration::from_secs(1)), Duration::from_secs(10));

    let mut headers = HeaderMap::new();
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("max-age=invalid, s-maxage=120"));
    assert_eq!(response_ttl(&headers, Duration::from_secs(1)), Duration::from_secs(120));

    let mut headers = HeaderMap::new();
    headers.insert(EXPIRES, HeaderValue::from_static("Wed, 21 Oct 2015 07:28:00 GMT"));
    assert_eq!(response_ttl(&headers, Duration::from_secs(60)), Duration::ZERO);
}
