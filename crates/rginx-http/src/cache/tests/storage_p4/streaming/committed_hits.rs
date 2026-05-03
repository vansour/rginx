use super::*;

#[tokio::test]
async fn committed_cache_hits_stream_body_from_file_in_multiple_frames() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 256 * 1024);
    let policy = test_policy();
    let request = Request::builder()
        .method(Method::GET)
        .uri("/committed-hit")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context = match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await
    {
        CacheLookup::Miss(context) => *context,
        _ => panic!("empty cache should miss"),
    };

    let expected_body = vec![b'x'; 160 * 1024];
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body(Bytes::from(expected_body.clone())))
        .expect("response should build");
    let stored = manager.store_response(context, response).await;
    let stored_body = drain_response(stored).await;
    assert_eq!(stored_body.len(), expected_body.len());

    let response = wait_for_hit(&manager, &request, &policy).await;
    assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
    let mut body = response.into_body();
    let first = timeout(Duration::from_millis(200), body.frame())
        .await
        .expect("first committed hit frame should arrive")
        .expect("committed hit body should yield a frame")
        .expect("first committed hit frame should read");
    let first_len = first.data_ref().expect("first frame should be data").len();
    assert!(first_len > 0 && first_len < expected_body.len());

    let second = timeout(Duration::from_millis(200), body.frame())
        .await
        .expect("second committed hit frame should arrive")
        .expect("committed hit body should yield a second frame")
        .expect("second committed hit frame should read");
    let second_len = second.data_ref().expect("second frame should be data").len();
    assert!(second_len > 0);

    let remaining = body.collect().await.unwrap().to_bytes();
    assert_eq!(first_len + second_len + remaining.len(), expected_body.len());
}

#[tokio::test]
async fn committed_slice_hits_stream_trimmed_body_from_file_in_multiple_frames() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 256 * 1024);
    let mut policy = test_policy();
    policy.range_requests = rginx_core::CacheRangeRequestPolicy::Cache;
    policy.slice_size_bytes = Some(160 * 1024);

    let request = Request::builder()
        .method(Method::GET)
        .uri("/committed-slice-hit")
        .header("host", "example.com")
        .header(RANGE, "bytes=0-99999")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context = match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await
    {
        CacheLookup::Miss(context) => *context,
        _ => panic!("empty cache should miss"),
    };

    let cached_slice_len = 160 * 1024;
    let response = Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header(CACHE_CONTROL, "max-age=60")
        .header(CONTENT_RANGE, format!("bytes 0-{}/{}", cached_slice_len - 1, cached_slice_len))
        .header(CONTENT_LENGTH, cached_slice_len.to_string())
        .body(full_body(Bytes::from(vec![b'y'; cached_slice_len])))
        .expect("response should build");
    let stored = manager.store_response(context, response).await;
    let stored_body = drain_response(stored).await;
    assert_eq!(stored_body.len(), 100_000);

    let response = wait_for_hit(&manager, &request, &policy).await;
    assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
    assert_eq!(
        response.headers().get(CONTENT_RANGE).unwrap().to_str().unwrap(),
        format!("bytes 0-99999/{cached_slice_len}")
    );
    assert_eq!(response.headers().get(CONTENT_LENGTH).unwrap(), "100000");
    let mut body = response.into_body();
    let first = timeout(Duration::from_millis(200), body.frame())
        .await
        .expect("first committed slice frame should arrive")
        .expect("committed slice body should yield a frame")
        .expect("first committed slice frame should read");
    let first_len = first.data_ref().expect("first committed slice frame should be data").len();
    assert!(first_len > 0 && first_len < 100_000);

    let second = timeout(Duration::from_millis(200), body.frame())
        .await
        .expect("second committed slice frame should arrive")
        .expect("committed slice body should yield a second frame")
        .expect("second committed slice frame should read");
    let second_len = second.data_ref().expect("second committed slice frame should be data").len();
    assert!(second_len > 0);

    let remaining = body.collect().await.unwrap().to_bytes();
    assert_eq!(first_len + second_len + remaining.len(), 100_000);
}
