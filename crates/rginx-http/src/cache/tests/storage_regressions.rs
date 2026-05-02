use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::header::{ACCEPT_ENCODING, ACCEPT_LANGUAGE, CACHE_CONTROL, ETAG};
use http::{Request, Response, StatusCode};
use http_body_util::BodyExt;
use tokio::time::{sleep, timeout};

use crate::handler::full_body;

use super::*;

#[tokio::test]
async fn cache_manager_matches_accept_encoding_vary_after_normalization() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();

    let gzip_br_request = Request::builder()
        .method(Method::GET)
        .uri("/vary-encoding")
        .header("host", "example.com")
        .header(ACCEPT_ENCODING, "GZip, br")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context = match manager
        .lookup(CacheRequest::from_request(&gzip_br_request), "https", &policy)
        .await
    {
        CacheLookup::Miss(context) => *context,
        _ => panic!("first encoding variant should miss"),
    };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .header("vary", "accept-encoding")
        .body(full_body("compressed"))
        .expect("response should build");
    let _ = drain_response(manager.store_response(context, response).await).await;

    let normalized_request = Request::builder()
        .method(Method::GET)
        .uri("/vary-encoding")
        .header("host", "example.com")
        .header(ACCEPT_ENCODING, "gzip,br")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    match manager.lookup(CacheRequest::from_request(&normalized_request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"compressed");
        }
        _ => panic!("normalized accept-encoding variant should hit"),
    }
}

#[test]
fn cache_variant_key_ignores_vary_header_order() {
    let accept_language = http::header::HeaderName::from_static("accept-language");
    let vary_left = vec![
        CachedVaryHeaderValue { name: accept_language.clone(), value: Some("zh-CN".to_string()) },
        CachedVaryHeaderValue { name: ACCEPT_ENCODING, value: Some("gzip,br".to_string()) },
    ];
    let vary_right = vec![vary_left[1].clone(), vary_left[0].clone()];

    assert_eq!(
        cache_variant_key("https:example.com:/variant", &vary_left),
        cache_variant_key("https:example.com:/variant", &vary_right),
    );
}

#[tokio::test]
async fn different_hashes_do_not_share_a_single_zone_io_lock() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let first_hash = cache_key_hash("https:example.com:/stripe-a");
    let second_hash = (0..256)
        .map(|suffix| cache_key_hash(&format!("https:example.com:/stripe-b-{suffix}")))
        .find(|hash| cache_io_lock_stripe(hash) != cache_io_lock_stripe(&first_hash))
        .expect("at least one candidate should land on a different stripe");

    let _first_guard = zone.io_write(&first_hash).await;
    timeout(Duration::from_millis(50), zone.io_read(&second_hash))
        .await
        .expect("different hashes should not be blocked by an unrelated hash lock");
}

#[tokio::test]
async fn refresh_not_modified_response_returns_merged_headers_when_no_cache_policy_matches() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let key = "https:example.com:/revalidate-no-cache";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let cached_metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        Response::builder()
            .status(StatusCode::OK)
            .header(CACHE_CONTROL, "max-age=60")
            .header(ETAG, "\"old\"")
            .body(())
            .expect("metadata response should build")
            .headers(),
        test_metadata_input(key, now.saturating_sub(2_000), now.saturating_sub(1_000), 6),
    );
    let paths = cache_paths(temp.path(), &hash);
    write_cache_entry(&paths, &cached_metadata, b"cached").await.expect("entry should be written");
    let cached_entry =
        test_index_entry(key, hash, 6, now.saturating_sub(1_000), now.saturating_sub(2_000));
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(key.to_string(), cached_entry.clone());
        index.current_size_bytes = 6;
    }

    let mut context = test_store_context(zone.clone(), key);
    context.policy.no_cache = Some(rginx_core::CachePredicate::Status(vec![StatusCode::OK]));
    context.cached_entry = Some(cached_entry);
    context.cached_metadata = Some(cached_metadata);
    context.cache_status = CacheStatus::Expired;

    let response = Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(CACHE_CONTROL, "max-age=10")
        .header(ETAG, "\"new\"")
        .body(full_body(Bytes::new()))
        .expect("304 response should build");
    let refreshed = refresh_not_modified_response(context, response)
        .await
        .expect("revalidation should reuse cached body");

    assert_eq!(refreshed.headers().get(CACHE_STATUS_HEADER).unwrap(), "REVALIDATED");
    assert_eq!(refreshed.headers().get(CACHE_CONTROL).unwrap(), "max-age=10");
    assert_eq!(refreshed.headers().get(ETAG).unwrap(), "\"new\"");
    let body = refreshed.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"cached");
    assert!(lock_index(&zone.index).entries.contains_key(key));
    assert!(paths.metadata.exists());
    assert!(paths.body.exists());
}

#[tokio::test]
async fn remove_cache_entry_if_matches_ignores_last_access_drift() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let key = "https:example.com:/last-access-drift";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        test_metadata_input(key, now.saturating_sub(2_000), now.saturating_add(60_000), 6),
    );
    let paths = cache_paths(temp.path(), &hash);
    write_cache_entry(&paths, &metadata, b"cached").await.expect("entry should be written");
    let cached_entry =
        test_index_entry(key, hash, 6, now.saturating_add(60_000), now.saturating_sub(2_000));
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(key.to_string(), cached_entry.clone());
        index.current_size_bytes = 6;
        index.entries.get_mut(key).expect("entry should exist").last_access_unix_ms = now;
    }

    assert!(remove_cache_entry_if_matches(&zone, key, &cached_entry).await);
    assert!(!lock_index(&zone.index).entries.contains_key(key));
    assert!(!paths.metadata.exists());
    assert!(!paths.body.exists());
}

#[tokio::test]
async fn rebucket_store_replaces_entry_after_last_access_update() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let zone = manager.zones.get("default").expect("default zone should exist").clone();
    let base_key = "https:example.com:/rebucket-last-access";
    let old_hash = cache_key_hash(base_key);
    let now = unix_time_ms(SystemTime::now());
    let old_metadata = cache_metadata(
        base_key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        test_metadata_input(base_key, now.saturating_sub(5_000), now.saturating_sub(1_000), 3),
    );
    let old_paths = cache_paths(temp.path(), &old_hash);
    write_cache_entry(&old_paths, &old_metadata, b"old").await.expect("entry should be written");
    let cached_entry = test_index_entry(
        base_key,
        old_hash,
        3,
        now.saturating_sub(1_000),
        now.saturating_sub(5_000),
    );
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(base_key.to_string(), cached_entry.clone());
        index.current_size_bytes = 3;
        index.entries.get_mut(base_key).expect("entry should exist").last_access_unix_ms = now;
    }

    let expected_final_key = cache_variant_key(
        base_key,
        &[CachedVaryHeaderValue { name: ACCEPT_LANGUAGE, value: Some("zh-CN".to_string()) }],
    );
    let mut context = test_store_context(zone.clone(), base_key);
    context.request.uri = http::Uri::from_static("/rebucket-last-access");
    context.request.headers.insert(ACCEPT_LANGUAGE, "zh-CN".parse().unwrap());
    context.cached_entry = Some(cached_entry);
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .header("vary", "accept-language")
        .body(full_body("new"))
        .expect("response should build");

    let stored = manager.store_response(context, response).await;
    assert_eq!(stored.headers().get(CACHE_STATUS_HEADER).unwrap(), "MISS");
    let _ = drain_response(stored).await;

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let rebucketed = {
                let index = lock_index(&zone.index);
                !index.entries.contains_key(base_key)
                    && index.entries.contains_key(&expected_final_key)
            };
            if rebucketed && !old_paths.metadata.exists() && !old_paths.body.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("rebucketed cache entry should replace the old index entry and files");
}

#[tokio::test]
async fn stale_removal_does_not_delete_in_flight_rewrite_for_same_hash() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path().to_path_buf(), 1024);
    let key = "https:example.com:/rewrite-race";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let cached_metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        Response::builder()
            .status(StatusCode::OK)
            .header(CACHE_CONTROL, "max-age=60")
            .body(())
            .expect("metadata response should build")
            .headers(),
        test_metadata_input(key, now.saturating_sub(2_000), now.saturating_add(60_000), 3),
    );
    let paths = cache_paths(temp.path(), &hash);
    write_cache_entry(&paths, &cached_metadata, b"old").await.expect("entry should be written");
    let cached_entry = test_index_entry(
        key,
        hash.clone(),
        3,
        now.saturating_add(60_000),
        now.saturating_sub(1_000),
    );
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(key.to_string(), cached_entry.clone());
        index.current_size_bytes = 3;
    }

    let new_metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        Response::builder()
            .status(StatusCode::OK)
            .header(CACHE_CONTROL, "max-age=120")
            .body(())
            .expect("metadata response should build")
            .headers(),
        test_metadata_input(key, now, now.saturating_add(120_000), 3),
    );
    let new_entry = test_index_entry(key, hash.clone(), 3, now.saturating_add(120_000), now);

    let io_guard = zone.io_write(&hash).await;
    write_cache_entry(&paths, &new_metadata, b"new")
        .await
        .expect("rewritten entry should be written while holding the hash lock");
    timeout(Duration::from_secs(1), async {
        loop {
            if tokio::fs::read(&paths.body).await.ok().is_some_and(|body| body.as_slice() == b"new")
            {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("rewritten cache files should be visible before the index swap");

    let removal_task = tokio::spawn({
        let zone = zone.clone();
        let key = key.to_string();
        let cached_entry = cached_entry.clone();
        async move { remove_cache_entry_if_matches(&zone, &key, &cached_entry).await }
    });
    sleep(Duration::from_millis(20)).await;
    assert!(!removal_task.is_finished(), "stale removal should wait for the hash write lock");

    {
        let _sync_guard = zone.shared_index_sync_lock.lock().await;
        let mut index = lock_index(&zone.index);
        index.insert_entry(key.to_string(), new_entry.clone());
    }
    drop(io_guard);

    assert!(
        !removal_task.await.expect("removal task should join"),
        "stale removal should not remove the rewritten entry"
    );

    let body = tokio::fs::read(&paths.body).await.expect("rewritten body should remain");
    assert_eq!(body.as_slice(), b"new");
    assert_eq!(
        read_cache_metadata(&paths.metadata).await.expect("metadata should decode").key,
        key
    );
    assert_eq!(lock_index(&zone.index).entries.get(key), Some(&new_entry));
}
