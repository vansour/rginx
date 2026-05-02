use super::*;

#[tokio::test]
async fn cache_manager_treats_corrupt_metadata_as_miss() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let key = "https:example.com:/broken";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    tokio::fs::create_dir_all(&paths.dir).await.expect("cache dir should be created");
    tokio::fs::write(&paths.metadata, b"{not-json").await.expect("metadata should be written");
    tokio::fs::write(&paths.body, b"cached").await.expect("body should be written");

    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            test_index_entry(
                key,
                hash,
                6,
                unix_time_ms(SystemTime::now()) + 60_000,
                unix_time_ms(SystemTime::now()),
            ),
        );
        index.current_size_bytes = 6;
    }

    let request = Request::builder()
        .method(Method::GET)
        .uri("/broken")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        CacheLookup::Hit(_) => panic!("corrupt metadata must not be served"),
        CacheLookup::Updating(_, _) => {
            panic!("corrupt metadata must not trigger background update")
        }
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }

    assert!(!lock_index(&zone.index).entries.contains_key(key));
}

#[tokio::test]
async fn cache_manager_treats_metadata_key_mismatch_as_miss() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let key = "https:example.com:/mismatch";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        "https:example.com:/other".to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        test_metadata_input(key, now.saturating_sub(2_000), now.saturating_add(60_000), 6),
    );
    write_cache_entry(&paths, &metadata, b"cached").await.expect("entry should be written");

    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            test_index_entry(key, hash, 6, now.saturating_add(60_000), now.saturating_sub(1_000)),
        );
        index.current_size_bytes = 6;
    }

    let request = Request::builder()
        .method(Method::GET)
        .uri("/mismatch")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        CacheLookup::Hit(_) => panic!("mismatched metadata key must not be served"),
        CacheLookup::Updating(_, _) => {
            panic!("mismatched metadata key must not trigger background update")
        }
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }

    assert!(!lock_index(&zone.index).entries.contains_key(key));
}

#[tokio::test]
async fn cache_manager_retains_expired_entries_for_revalidation_on_lookup() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let key = "https:example.com:/expired";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        test_metadata_input(key, now.saturating_sub(2_000), now.saturating_sub(1_000), 7),
    );
    write_cache_entry(&paths, &metadata, b"expired").await.expect("entry should be written");
    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            test_index_entry(
                key,
                hash.clone(),
                7,
                now.saturating_sub(1_000),
                now.saturating_sub(2_000),
            ),
        );
        index.current_size_bytes = 7;
    }

    let request = Request::builder()
        .method(Method::GET)
        .uri("/expired")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(context) => assert_eq!(context.cache_status(), CacheStatus::Expired),
        CacheLookup::Hit(_) => panic!("expired response must not be served"),
        CacheLookup::Updating(_, _) => panic!("expired response should not update without policy"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }

    let index = lock_index(&zone.index);
    assert!(index.entries.contains_key(key));
    assert_eq!(index.current_size_bytes, 7);
    assert!(paths.metadata.exists());
    assert!(paths.body.exists());
}

#[tokio::test]
async fn cache_manager_serves_head_hit_without_reading_body_file() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let get_request = Request::builder()
        .method(Method::GET)
        .uri("/head")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context = match manager
        .lookup(CacheRequest::from_request(&get_request), "https", &policy)
        .await
    {
        CacheLookup::Miss(context) => *context,
        CacheLookup::Hit(_) => panic!("empty cache should miss"),
        CacheLookup::Updating(_, _) => panic!("empty cache should not update"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    };
    let response = http::Response::builder()
        .status(StatusCode::OK)
        .body(full_body("cached body"))
        .expect("response should build");
    let _ = drain_response(manager.store_response(context, response).await).await;

    let hash = cache_key_hash("https:example.com:/head");
    let paths = cache_paths(temp.path(), &hash);
    tokio::time::timeout(Duration::from_secs(1), async {
        while !paths.body.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("body file should appear after streaming cache store completes");
    tokio::fs::remove_file(paths.body).await.expect("body file should be removable");

    let head_request = Request::builder()
        .method(Method::HEAD)
        .uri("/head")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    match manager.lookup(CacheRequest::from_request(&head_request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "HIT");
            assert_eq!(response.headers().get(http::header::CONTENT_LENGTH).unwrap(), "11");
            let body =
                http_body_util::BodyExt::collect(response.into_body()).await.unwrap().to_bytes();
            assert!(body.is_empty());
        }
        CacheLookup::Miss(_) => panic!("HEAD should not need the cached body file"),
        CacheLookup::Updating(_, _) => panic!("HEAD hit should not trigger background update"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }
}

#[tokio::test]
async fn cache_manager_treats_missing_body_file_as_miss_and_cleans_index() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let key = "https:example.com:/missing-body";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        test_metadata_input(key, now.saturating_sub(2_000), now.saturating_add(60_000), 6),
    );
    write_cache_entry(&paths, &metadata, b"cached").await.expect("entry should be written");

    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            test_index_entry(key, hash, 6, now.saturating_add(60_000), now),
        );
        index.current_size_bytes = 6;
    }

    tokio::fs::remove_file(&paths.body).await.expect("body file should be removable");

    let request = Request::builder()
        .method(Method::GET)
        .uri("/missing-body")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        CacheLookup::Hit(_) => panic!("missing body file must not be served"),
        CacheLookup::Updating(_, _) => {
            panic!("missing body file must not trigger background update")
        }
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }

    let index = lock_index(&zone.index);
    assert!(!index.entries.contains_key(key));
    assert_eq!(index.current_size_bytes, 0);
    drop(index);
    assert!(!paths.metadata.exists());
    assert!(!paths.body.exists());
}

#[test]
fn should_refresh_from_not_modified_requires_cached_entry() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let mut context = test_store_context(zone, "/revalidate");

    assert!(!context.should_refresh_from_not_modified(StatusCode::NOT_MODIFIED));

    context.cached_entry = Some(test_index_entry("/revalidate", "hash".to_string(), 6, 0, 0));
    assert!(context.should_refresh_from_not_modified(StatusCode::NOT_MODIFIED));
    assert!(!context.should_refresh_from_not_modified(StatusCode::OK));
}

#[test]
fn no_store_headers_remain_non_storable_during_revalidation() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let context = test_store_context(zone, "/");
    let response = http::Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "no-store")
        .body(full_body("hello"))
        .expect("response should build");

    assert!(!response_is_storable(&context, &response));
}

#[tokio::test]
async fn fresh_no_cache_entry_revalidates_before_hit() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let key = "https:example.com:/fresh-no-cache";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        CacheMetadataInput {
            requires_revalidation: true,
            ..test_metadata_input(key, now.saturating_sub(2_000), now.saturating_add(60_000), 6)
        },
    );
    write_cache_entry(&paths, &metadata, b"cached").await.expect("entry should be written");

    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            CacheIndexEntry {
                requires_revalidation: true,
                ..test_index_entry(key, hash, 6, now.saturating_add(60_000), now)
            },
        );
        index.current_size_bytes = 6;
    }

    let request = Request::builder()
        .method(Method::GET)
        .uri("/fresh-no-cache")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(context) => assert_eq!(context.cache_status(), CacheStatus::Revalidated),
        _ => panic!("fresh no-cache entry should revalidate before it can hit"),
    }
}

#[tokio::test]
async fn fresh_must_revalidate_entry_still_hits_until_expiry() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let key = "https:example.com:/fresh-must-revalidate";
    let hash = cache_key_hash(key);
    let paths = cache_paths(temp.path(), &hash);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        CacheMetadataInput {
            must_revalidate: true,
            ..test_metadata_input(key, now.saturating_sub(2_000), now.saturating_add(60_000), 6)
        },
    );
    write_cache_entry(&paths, &metadata, b"cached").await.expect("entry should be written");

    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            CacheIndexEntry {
                must_revalidate: true,
                ..test_index_entry(key, hash, 6, now.saturating_add(60_000), now)
            },
        );
        index.current_size_bytes = 6;
    }

    let request = Request::builder()
        .method(Method::GET)
        .uri("/fresh-must-revalidate")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(_) => {}
        _ => panic!("fresh must-revalidate entry should remain cacheable until it expires"),
    }
}
