use super::*;

#[test]
fn cache_key_reuses_slice_for_subranges_within_same_slice() {
    let mut policy = test_policy();
    policy.range_requests = rginx_core::CacheRangeRequestPolicy::Cache;
    policy.slice_size_bytes = Some(128);
    let first = Request::builder()
        .method(Method::GET)
        .uri("/video.mp4")
        .header("host", "example.com")
        .header(http::header::RANGE, "bytes=2-4")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let second = Request::builder()
        .method(Method::GET)
        .uri("/video.mp4")
        .header("host", "example.com")
        .header(http::header::RANGE, "bytes=5-6")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    assert_eq!(
        render_cache_key(first.method(), first.uri(), first.headers(), "https", &policy),
        "https:example.com:/video.mp4|range:0-127"
    );
    assert_eq!(
        render_cache_key(second.method(), second.uri(), second.headers(), "https", &policy),
        "https:example.com:/video.mp4|range:0-127"
    );
}

#[test]
fn cross_slice_range_bypasses_when_slice_size_is_configured() {
    let mut policy = test_policy();
    policy.methods = vec![Method::GET];
    policy.key = rginx_core::CacheKeyTemplate::parse("{uri}").expect("key should parse");
    policy.range_requests = rginx_core::CacheRangeRequestPolicy::Cache;
    policy.slice_size_bytes = Some(8);
    let request = Request::builder()
        .method(Method::GET)
        .uri("/video.mp4")
        .header(http::header::RANGE, "bytes=6-9")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    assert!(cache_request_bypass(&CacheRequest::from_request(&request), &policy));
}
