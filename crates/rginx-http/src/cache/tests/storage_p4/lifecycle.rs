use std::time::{Duration, SystemTime};

use bytes::Bytes;
use http::header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, RANGE, SET_COOKIE};
use http::{Method, Request, Response, StatusCode};
use http_body_util::BodyExt;

use crate::handler::full_body;

use super::*;

mod pass_markers;

#[tokio::test]
async fn expired_entry_in_grace_window_serves_stale_while_updating() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.background_update = true;
    policy.use_stale = vec![rginx_core::CacheUseStaleCondition::Updating];

    let key = "https:example.com:/grace-updating";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let paths = cache_paths(temp.path(), &hash);
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        Response::builder()
            .status(StatusCode::OK)
            .header(CACHE_CONTROL, "max-age=60")
            .body(())
            .expect("metadata response should build")
            .headers(),
        CacheMetadataInput {
            grace_until_unix_ms: Some(now.saturating_add(60_000)),
            keep_until_unix_ms: Some(now.saturating_add(120_000)),
            ..test_metadata_input(key, now.saturating_sub(2_000), now.saturating_sub(1_000), 6)
        },
    );
    write_cache_entry(&paths, &metadata, b"cached").await.expect("entry should be written");
    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            CacheIndexEntry {
                grace_until_unix_ms: Some(now.saturating_add(60_000)),
                keep_until_unix_ms: Some(now.saturating_add(120_000)),
                ..test_index_entry(
                    key,
                    hash,
                    6,
                    now.saturating_sub(1_000),
                    now.saturating_sub(2_000),
                )
            },
        );
        index.current_size_bytes = 6;
    }

    let request = Request::builder()
        .method(Method::GET)
        .uri("/grace-updating")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Updating(response, context) => {
            assert_eq!(response.headers().get(CACHE_STATUS_HEADER).unwrap(), "UPDATING");
            assert_eq!(context.cache_status(), CacheStatus::Updating);
        }
        _ => panic!("grace-window entry should serve stale while background updating"),
    }
}

#[tokio::test]
async fn expired_entry_in_keep_window_revalidates_without_stale_serve() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.background_update = true;
    policy.use_stale = vec![rginx_core::CacheUseStaleCondition::Updating];

    let key = "https:example.com:/keep-revalidate";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let paths = cache_paths(temp.path(), &hash);
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        Response::builder()
            .status(StatusCode::OK)
            .header(CACHE_CONTROL, "max-age=60")
            .body(())
            .expect("metadata response should build")
            .headers(),
        CacheMetadataInput {
            grace_until_unix_ms: Some(now.saturating_sub(500)),
            keep_until_unix_ms: Some(now.saturating_add(60_000)),
            ..test_metadata_input(key, now.saturating_sub(2_000), now.saturating_sub(1_000), 6)
        },
    );
    write_cache_entry(&paths, &metadata, b"cached").await.expect("entry should be written");
    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            CacheIndexEntry {
                grace_until_unix_ms: Some(now.saturating_sub(500)),
                keep_until_unix_ms: Some(now.saturating_add(60_000)),
                ..test_index_entry(
                    key,
                    hash,
                    6,
                    now.saturating_sub(1_000),
                    now.saturating_sub(2_000),
                )
            },
        );
        index.current_size_bytes = 6;
    }

    let request = Request::builder()
        .method(Method::GET)
        .uri("/keep-revalidate")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(context) => {
            assert_eq!(context.cache_status(), CacheStatus::Expired);
            assert!(context.cached_entry.is_some(), "keep-window entry should stay revalidatable");
        }
        _ => panic!("post-grace keep-window entry should revalidate instead of serving stale"),
    }

    let index = lock_index(&zone.index);
    assert!(index.entries.contains_key(key));
    drop(index);
    assert!(paths.metadata.exists());
    assert!(paths.body.exists());
}

#[tokio::test]
async fn expired_entry_past_keep_window_is_dropped_before_next_fill() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();

    let key = "https:example.com:/keep-dead";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let paths = cache_paths(temp.path(), &hash);
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        Response::builder()
            .status(StatusCode::OK)
            .header(CACHE_CONTROL, "max-age=60")
            .body(())
            .expect("metadata response should build")
            .headers(),
        CacheMetadataInput {
            grace_until_unix_ms: Some(now.saturating_sub(1_000)),
            keep_until_unix_ms: Some(now.saturating_sub(500)),
            ..test_metadata_input(key, now.saturating_sub(3_000), now.saturating_sub(2_000), 6)
        },
    );
    write_cache_entry(&paths, &metadata, b"cached").await.expect("entry should be written");
    let zone = manager.zones.get("default").expect("zone should exist");
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            CacheIndexEntry {
                grace_until_unix_ms: Some(now.saturating_sub(1_000)),
                keep_until_unix_ms: Some(now.saturating_sub(500)),
                ..test_index_entry(
                    key,
                    hash,
                    6,
                    now.saturating_sub(2_000),
                    now.saturating_sub(3_000),
                )
            },
        );
        index.current_size_bytes = 6;
    }

    let request = Request::builder()
        .method(Method::GET)
        .uri("/keep-dead")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(context) => assert_eq!(context.cache_status(), CacheStatus::Miss),
        _ => panic!("dead entry should be removed and fall through to a normal miss"),
    }

    let index = lock_index(&zone.index);
    assert!(!index.entries.contains_key(key));
    assert_eq!(index.current_size_bytes, 0);
    drop(index);
    assert!(!paths.metadata.exists());
    assert!(!paths.body.exists());
}

#[tokio::test]
async fn set_cookie_response_creates_hit_for_pass_and_bypasses_followup_lookup() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let mut policy = test_policy();
    policy.pass_ttl = Some(Duration::from_secs(30));

    let request = Request::builder()
        .method(Method::GET)
        .uri("/pass-cookie")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    let context = match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await
    {
        CacheLookup::Miss(context) => *context,
        _ => panic!("empty cache should miss"),
    };

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .header(SET_COOKIE, "sid=abc")
        .body(full_body("private"))
        .expect("response should build");
    let stored = manager.store_response(context, response).await;
    let body = stored.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(body.as_ref(), b"private");

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Bypass(status) => assert_eq!(status, CacheStatus::Bypass),
        _ => panic!("hit-for-pass marker should bypass follow-up lookup"),
    }

    let key = "https:example.com:/pass-cookie";
    let pass_hash = cache_key_hash(&format!("pass:{key}"));
    let zone = manager.zones.get("default").expect("zone should exist");
    let index = lock_index(&zone.index);
    let entry = index.entries.get(key).expect("pass marker should be indexed");
    assert!(entry.is_hit_for_pass());
    assert_eq!(entry.hash, pass_hash);
    assert_eq!(entry.body_size_bytes, 0);
    drop(index);

    let paths = cache_paths(temp.path(), &cache_key_hash(key));
    assert!(!paths.metadata.exists());
    assert!(!paths.body.exists());
}
