use super::*;

#[test]
fn head_cache_key_is_separated_when_convert_head_is_disabled() {
    let mut policy = test_policy();
    policy.convert_head = false;
    let get_request = Request::builder()
        .method(Method::GET)
        .uri("/head-key")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let head_request = Request::builder()
        .method(Method::HEAD)
        .uri("/head-key")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    let get_key = render_cache_key(
        get_request.method(),
        get_request.uri(),
        get_request.headers(),
        "https",
        &policy,
    );
    let head_key = render_cache_key(
        head_request.method(),
        head_request.uri(),
        head_request.headers(),
        "https",
        &policy,
    );

    assert_eq!(get_key, "https:example.com:/head-key|cache-method:GET");
    assert_eq!(head_key, "https:example.com:/head-key|cache-method:HEAD");

    policy.convert_head = true;
    let shared_head_key = render_cache_key(
        head_request.method(),
        head_request.uri(),
        head_request.headers(),
        "https",
        &policy,
    );
    assert_eq!(shared_head_key, "https:example.com:/head-key");
}
