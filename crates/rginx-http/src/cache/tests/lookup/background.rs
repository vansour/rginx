use super::*;

#[tokio::test]
async fn cache_manager_returns_updating_for_background_refresh() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.background_update = true;
    policy.use_stale = vec![rginx_core::CacheUseStaleCondition::Updating];

    let key = "https:example.com:/background";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        CacheMetadataInput {
            grace_until_unix_ms: Some(now.saturating_add(60_000)),
            keep_until_unix_ms: Some(now.saturating_add(60_000)),
            ..test_metadata_input(key, now.saturating_sub(2_000), now.saturating_sub(1_000), 7)
        },
    );
    write_cache_entry(&paths, &metadata, b"cached!").await.expect("entry should be written");
    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            CacheIndexEntry {
                grace_until_unix_ms: Some(now.saturating_add(60_000)),
                keep_until_unix_ms: Some(now.saturating_add(60_000)),
                ..test_index_entry(
                    key,
                    hash.clone(),
                    7,
                    now.saturating_sub(1_000),
                    now.saturating_sub(2_000),
                )
            },
        );
        index.current_size_bytes = 7;
    }

    let request = Request::builder()
        .method(Method::GET)
        .uri("/background")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Updating(response, context) => {
            assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "UPDATING");
            assert_eq!(context.cache_status(), CacheStatus::Updating);
        }
        _ => panic!("expected background update"),
    }
}

#[tokio::test]
async fn cache_manager_does_not_background_update_when_request_forces_revalidation() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.background_update = true;
    policy.use_stale = vec![rginx_core::CacheUseStaleCondition::Updating];

    let key = "https:example.com:/background-no-cache";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        test_metadata_input(key, now.saturating_sub(2_000), now.saturating_sub(1_000), 7),
    );
    write_cache_entry(&paths, &metadata, b"cached!").await.expect("entry should be written");
    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            test_index_entry(key, hash, 7, now.saturating_sub(1_000), now.saturating_sub(2_000)),
        );
        index.current_size_bytes = 7;
    }

    let request = Request::builder()
        .method(Method::GET)
        .uri("/background-no-cache")
        .header("host", "example.com")
        .header(CACHE_CONTROL, "no-cache")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(context) => assert_eq!(context.cache_status(), CacheStatus::Expired),
        _ => panic!("forced revalidation should not be converted into background stale serve"),
    }
}

#[tokio::test]
async fn cache_manager_does_not_background_update_must_revalidate_entries() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.background_update = true;
    policy.use_stale = vec![rginx_core::CacheUseStaleCondition::Updating];

    let key = "https:example.com:/background-must-revalidate";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        CacheMetadataInput {
            stale_while_revalidate_until_unix_ms: Some(now.saturating_add(60_000)),
            must_revalidate: true,
            ..test_metadata_input(key, now.saturating_sub(2_000), now.saturating_sub(1_000), 7)
        },
    );
    write_cache_entry(&paths, &metadata, b"cached!").await.expect("entry should be written");
    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            CacheIndexEntry {
                stale_while_revalidate_until_unix_ms: Some(now.saturating_add(60_000)),
                must_revalidate: true,
                ..test_index_entry(
                    key,
                    hash,
                    7,
                    now.saturating_sub(1_000),
                    now.saturating_sub(2_000),
                )
            },
        );
        index.current_size_bytes = 7;
    }

    let request = Request::builder()
        .method(Method::GET)
        .uri("/background-must-revalidate")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(context) => assert_eq!(context.cache_status(), CacheStatus::Expired),
        _ => panic!("must-revalidate entries should not be served stale while updating"),
    }
}

#[tokio::test]
async fn head_background_refresh_stores_revalidated_body() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.background_update = true;
    policy.use_stale = vec![rginx_core::CacheUseStaleCondition::Updating];

    let key = "https:example.com:/head-refresh";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        CacheMetadataInput {
            grace_until_unix_ms: Some(now.saturating_add(60_000)),
            keep_until_unix_ms: Some(now.saturating_add(60_000)),
            ..test_metadata_input(key, now.saturating_sub(2_000), now.saturating_sub(1_000), 3)
        },
    );
    write_cache_entry(&paths, &metadata, b"old").await.expect("entry should be written");
    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            CacheIndexEntry {
                grace_until_unix_ms: Some(now.saturating_add(60_000)),
                keep_until_unix_ms: Some(now.saturating_add(60_000)),
                ..test_index_entry(
                    key,
                    hash,
                    3,
                    now.saturating_sub(1_000),
                    now.saturating_sub(2_000),
                )
            },
        );
        index.current_size_bytes = 3;
    }

    let request = Request::builder()
        .method(Method::HEAD)
        .uri("/head-refresh")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context = match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await
    {
        CacheLookup::Updating(_, context) => *context,
        other => panic!(
            "expected updating cache lookup, got {}",
            match other {
                CacheLookup::Hit(_) => "hit",
                CacheLookup::Miss(_) => "miss",
                CacheLookup::Updating(_, _) => "updating",
                CacheLookup::Bypass(_) => "bypass",
            }
        ),
    };
    assert_eq!(context.request.method, Method::GET);
    assert!(context.store_response);

    let response = http::Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .body(full_body("fresh"))
        .expect("response should build");
    let _ = drain_response(manager.store_response(context, response).await).await;

    let get_request = Request::builder()
        .method(Method::GET)
        .uri("/head-refresh")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            match manager.lookup(CacheRequest::from_request(&get_request), "https", &policy).await {
                CacheLookup::Hit(response) => {
                    let body = http_body_util::BodyExt::collect(response.into_body())
                        .await
                        .expect("body should collect")
                        .to_bytes();
                    if body.as_ref() == b"fresh" {
                        break;
                    }
                }
                CacheLookup::Miss(_) | CacheLookup::Updating(_, _) => {}
                CacheLookup::Bypass(status) => {
                    panic!("cacheable request should not bypass: {status:?}")
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("background refresh from HEAD should update cached body");
}

#[tokio::test]
async fn cache_manager_lock_timeout_falls_back_to_bypass() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.lock_timeout = Duration::from_millis(1);
    let key = "https:example.com:/locked";
    let zone = manager.zones.get("default").expect("zone should exist");
    zone.fill_locks.lock().unwrap().insert(
        key.to_string(),
        CacheFillLockState {
            notify: Arc::new(Notify::new()),
            acquired_at_unix_ms: unix_time_ms(SystemTime::now()),
            generation: 1,
            share_fingerprint: String::new(),
            reader_state: None,
        },
    );

    let request = Request::builder()
        .method(Method::GET)
        .uri("/locked")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(context) => {
            assert_eq!(context.cache_status(), CacheStatus::Bypass);
        }
        _ => panic!("expected timeout bypass miss"),
    }
}

#[tokio::test]
async fn cache_manager_lock_age_allows_second_fill() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.lock_age = Duration::from_millis(1);
    let key = "https:example.com:/aged";
    let zone = manager.zones.get("default").expect("zone should exist");
    zone.fill_locks.lock().unwrap().insert(
        key.to_string(),
        CacheFillLockState {
            notify: Arc::new(Notify::new()),
            acquired_at_unix_ms: unix_time_ms(SystemTime::now()).saturating_sub(5_000),
            generation: 1,
            share_fingerprint: String::new(),
            reader_state: None,
        },
    );

    let request = Request::builder()
        .method(Method::GET)
        .uri("/aged")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(context) => assert_eq!(context.cache_status(), CacheStatus::Miss),
        _ => panic!("expected fresh fill acquisition"),
    }
}
