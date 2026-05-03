use std::time::Duration;

use bytes::Bytes;
use http::header::CACHE_CONTROL;
use http::{Method, Request, Response, StatusCode};
use http_body_util::BodyExt;

use crate::handler::full_body;

use super::*;

fn cache_request(uri: &'static str) -> Request<crate::handler::HttpBody> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build")
}

fn cache_key(uri: &str) -> String {
    format!("https:example.com:{uri}")
}

async fn store_response_with_headers(
    manager: &CacheManager,
    request: &Request<crate::handler::HttpBody>,
    policy: &RouteCachePolicy,
    extra_headers: &[(&str, &str)],
    body: &'static str,
) {
    let context = match manager.lookup(CacheRequest::from_request(request), "https", policy).await {
        CacheLookup::Miss(context) => *context,
        CacheLookup::Hit(_) => panic!("cache should miss before storing"),
        CacheLookup::Updating(_, _) => panic!("cache should not background update before storing"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    };

    let mut builder =
        Response::builder().status(StatusCode::OK).header(CACHE_CONTROL, "max-age=60");
    for (name, value) in extra_headers {
        builder = builder.header(*name, *value);
    }
    let response = builder.body(full_body(body)).expect("response should build");
    let _ = drain_response(manager.store_response(context, response).await).await;
}

#[tokio::test]
async fn invalidate_tag_is_lazy_and_lookup_cleans_files() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = cache_request("/tagged");
    let key = cache_key("/tagged");
    let hash = cache_key_hash(&key);
    let paths = cache_paths(temp.path(), &hash);

    store_response_with_headers(
        &manager,
        &request,
        &policy,
        &[("cache-tag", " News, Sports ")],
        "tagged",
    )
    .await;
    let _ = wait_for_hit(&manager, &request, &policy).await;

    let invalidation =
        manager.invalidate_tag("default", "  NEWS ").await.expect("tag invalidation should work");
    assert_eq!(invalidation.scope, "tag:news");
    assert_eq!(invalidation.affected_entries, 1);
    assert_eq!(invalidation.affected_bytes, 6);
    assert_eq!(invalidation.active_rules, 1);
    assert!(paths.metadata.exists(), "logical invalidation should not eagerly delete metadata");
    assert!(paths.body.exists(), "logical invalidation should not eagerly delete body");

    let snapshot = manager.snapshot();
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].invalidation_total, 1);
    assert_eq!(snapshot[0].active_invalidation_rules, 1);

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(context) => assert_eq!(context.cache_status(), CacheStatus::Miss),
        CacheLookup::Hit(_) => panic!("invalidated entry must not be served"),
        CacheLookup::Updating(_, _) => panic!("invalidated entry must not trigger background hit"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }

    let zone = manager.zones.get("default").expect("zone should exist");
    assert!(!lock_index(&zone.index).entries.contains_key(&key));
    assert!(!paths.metadata.exists());
    assert!(!paths.body.exists());
}

#[tokio::test]
async fn invalidation_rule_does_not_match_newer_entry_with_same_tag() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = cache_request("/tag-reload");

    store_response_with_headers(&manager, &request, &policy, &[("surrogate-key", "news")], "stale")
        .await;
    let _ = wait_for_hit(&manager, &request, &policy).await;

    let invalidation =
        manager.invalidate_tag("default", "news").await.expect("tag invalidation should work");
    assert_eq!(invalidation.affected_entries, 1);
    tokio::time::sleep(Duration::from_millis(2)).await;

    let refill_context = match manager
        .lookup(CacheRequest::from_request(&request), "https", &policy)
        .await
    {
        CacheLookup::Miss(context) => *context,
        CacheLookup::Hit(_) => panic!("invalidated entry must not hit before refill"),
        CacheLookup::Updating(_, _) => panic!("invalidated entry must not update before refill"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    };
    let response = Response::builder()
        .status(StatusCode::OK)
        .header(CACHE_CONTROL, "max-age=60")
        .header("x-cache-tag", "News")
        .body(full_body("fresh"))
        .expect("response should build");
    let _ = drain_response(manager.store_response(refill_context, response).await).await;

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"fresh");
        }
        CacheLookup::Miss(_) => panic!("newer entry must not be blocked by older invalidation"),
        CacheLookup::Updating(_, _) => panic!("fresh entry must not background update"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }

    let snapshot = manager.snapshot();
    assert_eq!(snapshot[0].active_invalidation_rules, 1);
}

#[tokio::test]
async fn clear_invalidations_restores_visibility_before_lazy_delete() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = cache_request("/tag-clear");

    store_response_with_headers(&manager, &request, &policy, &[("cache-tag", "news")], "restore")
        .await;
    let _ = wait_for_hit(&manager, &request, &policy).await;

    let invalidation =
        manager.invalidate_tag("default", "news").await.expect("tag invalidation should work");
    assert_eq!(invalidation.active_rules, 1);

    let cleared =
        manager.clear_invalidations("default").await.expect("clear invalidations should work");
    assert_eq!(cleared.scope, "clear");
    assert_eq!(cleared.affected_entries, 1);
    assert_eq!(cleared.active_rules, 0);

    match manager.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"restore");
        }
        CacheLookup::Miss(_) => panic!("cleared invalidations should restore the cached entry"),
        CacheLookup::Updating(_, _) => panic!("restored entry must not update"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }

    let snapshot = manager.snapshot();
    assert_eq!(snapshot[0].active_invalidation_rules, 0);
}

#[tokio::test]
async fn invalidate_prefix_is_lazy_and_does_not_touch_unmatched_entries() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let invalidated_a = cache_request("/prefix/a");
    let invalidated_b = cache_request("/prefix/b");
    let retained = cache_request("/other");
    let key_a = cache_key("/prefix/a");
    let key_b = cache_key("/prefix/b");
    let hash_a = cache_key_hash(&key_a);
    let hash_b = cache_key_hash(&key_b);
    let paths_a = cache_paths(temp.path(), &hash_a);
    let paths_b = cache_paths(temp.path(), &hash_b);

    store_response_with_headers(&manager, &invalidated_a, &policy, &[], "aaa").await;
    store_response_with_headers(&manager, &invalidated_b, &policy, &[], "bbb").await;
    store_response_with_headers(&manager, &retained, &policy, &[], "keep").await;
    let _ = wait_for_hit(&manager, &invalidated_a, &policy).await;
    let _ = wait_for_hit(&manager, &invalidated_b, &policy).await;
    let _ = wait_for_hit(&manager, &retained, &policy).await;

    let invalidation = manager
        .invalidate_prefix("default", "https:example.com:/prefix/")
        .await
        .expect("prefix invalidation should work");
    assert_eq!(invalidation.scope, "prefix:https:example.com:/prefix/");
    assert_eq!(invalidation.affected_entries, 2);
    assert!(paths_a.metadata.exists());
    assert!(paths_b.metadata.exists());

    match manager.lookup(CacheRequest::from_request(&retained), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"keep");
        }
        _ => panic!("unmatched entry should remain visible"),
    }

    match manager.lookup(CacheRequest::from_request(&invalidated_a), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        _ => panic!("prefix-invalidated entry should miss"),
    }

    let zone = manager.zones.get("default").expect("zone should exist");
    let index = lock_index(&zone.index);
    assert!(!index.entries.contains_key(&key_a));
    assert!(index.entries.contains_key(&key_b));
    drop(index);
    assert!(!paths_a.metadata.exists());
    assert!(!paths_a.body.exists());
    assert!(paths_b.metadata.exists());
    assert!(paths_b.body.exists());
}

#[tokio::test]
async fn shared_index_sync_propagates_tag_invalidation_between_managers() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager_a = test_manager(temp.path().to_path_buf(), 1024);
    let manager_b = test_manager(temp.path().to_path_buf(), 1024);
    let policy = test_policy();
    let request = cache_request("/shared-tag");
    let key = cache_key("/shared-tag");
    let hash = cache_key_hash(&key);
    let paths = cache_paths(temp.path(), &hash);

    store_response_with_headers(&manager_a, &request, &policy, &[("cache-tag", "news")], "shared")
        .await;
    let _ = wait_for_hit(&manager_a, &request, &policy).await;

    match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"shared");
        }
        _ => panic!("second manager should hit shared entry before invalidation"),
    }

    let zone_b = manager_b.zones.get("default").expect("default zone should exist");
    assert!(zone_b.prepared_response_head(&key, &hash).is_some());

    let invalidation =
        manager_a.invalidate_tag("default", "news").await.expect("tag invalidation should work");
    assert_eq!(invalidation.affected_entries, 1);

    match manager_b.lookup(CacheRequest::from_request(&request), "https", &policy).await {
        CacheLookup::Miss(_) => {}
        CacheLookup::Hit(_) => panic!("shared invalidation must stop serving the entry"),
        CacheLookup::Updating(_, _) => panic!("shared invalidation must not background update"),
        CacheLookup::Bypass(status) => panic!("cacheable request should not bypass: {status:?}"),
    }

    assert!(zone_b.prepared_response_head(&key, &hash).is_none());
    assert!(!paths.metadata.exists());
    assert!(!paths.body.exists());
}
