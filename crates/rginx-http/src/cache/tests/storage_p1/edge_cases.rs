use super::*;

#[tokio::test]
async fn cleanup_inactive_entries_honors_manager_batch_entries() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager_sleep = Duration::from_millis(20);
    let config = Arc::new(CacheZone {
        name: "default".to_string(),
        path: temp.path().to_path_buf(),
        max_size_bytes: Some(1024 * 1024),
        inactive: Duration::from_secs(1),
        default_ttl: Duration::from_secs(60),
        max_entry_bytes: 1024,
        path_levels: vec![2],
        loader_batch_entries: 100,
        loader_sleep: Duration::ZERO,
        manager_batch_entries: 1,
        manager_sleep,
        inactive_cleanup_interval: Duration::ZERO,
        shared_index: true,
    });
    let (
        index,
        shared_index_store,
        shared_index_generation,
        shared_index_store_epoch,
        shared_index_change_seq,
    ) = shared::bootstrap_shared_index(config.as_ref())
        .expect("test shared index should bootstrap");
    let zone = Arc::new(CacheZoneRuntime {
        config: config.clone(),
        index: RwLock::new(index),
        hot_entries: RwLock::new(HashMap::new()),
        io_locks: CacheIoLockPool::new(),
        shared_index_sync_lock: AsyncMutex::new(()),
        shared_index_store,
        fill_locks: Arc::new(Mutex::new(HashMap::new())),
        fill_lock_generation: AtomicU64::new(0),
        last_inactive_cleanup_unix_ms: AtomicU64::new(0),
        shared_index_generation: AtomicU64::new(shared_index_generation),
        shared_index_store_epoch: AtomicU64::new(shared_index_store_epoch),
        shared_index_change_seq: AtomicU64::new(shared_index_change_seq),
        stats: CacheZoneStats::default(),
        change_notifier: None,
    });
    let now = unix_time_ms(SystemTime::now());

    for (suffix, body) in [("one", b"one".as_slice()), ("two", b"two".as_slice())] {
        let key = format!("https:example.com:/{suffix}");
        let hash = cache_key_hash(&key);
        let metadata = cache_metadata(
            key.clone(),
            StatusCode::OK,
            &http::HeaderMap::new(),
            test_metadata_input(&key, now.saturating_sub(5_000), now.saturating_sub(2_000), 3),
        );
        let paths = cache_paths_for_zone(zone.config.as_ref(), &hash);
        write_cache_entry(&paths, &metadata, body).await.expect("entry should be written");
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.clone(),
            test_index_entry(&key, hash, 3, now.saturating_sub(2_000), now.saturating_sub(5_000)),
        );
        index.current_size_bytes += 3;
    }

    let started = Instant::now();
    cleanup_inactive_entries_in_zone(&zone).await;
    assert!(lock_index(&zone.index).entries.is_empty());
    assert!(started.elapsed() >= manager_sleep, "cleanup should pause between batches");
}

#[tokio::test]
async fn recent_hit_keeps_entry_out_of_inactive_cleanup_without_rewriting_metadata() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let config = Arc::new(CacheZone {
        name: "default".to_string(),
        path: temp.path().to_path_buf(),
        max_size_bytes: Some(1024 * 1024),
        inactive: Duration::from_secs(1),
        default_ttl: Duration::from_secs(60),
        max_entry_bytes: 1024,
        path_levels: vec![2],
        loader_batch_entries: 100,
        loader_sleep: Duration::ZERO,
        manager_batch_entries: 100,
        manager_sleep: Duration::ZERO,
        inactive_cleanup_interval: Duration::ZERO,
        shared_index: true,
    });
    let (
        index,
        shared_index_store,
        shared_index_generation,
        shared_index_store_epoch,
        shared_index_change_seq,
    ) = shared::bootstrap_shared_index(config.as_ref())
        .expect("test shared index should bootstrap");
    let zone = Arc::new(CacheZoneRuntime {
        config: config.clone(),
        index: RwLock::new(index),
        hot_entries: RwLock::new(HashMap::new()),
        io_locks: CacheIoLockPool::new(),
        shared_index_sync_lock: AsyncMutex::new(()),
        shared_index_store,
        fill_locks: Arc::new(Mutex::new(HashMap::new())),
        fill_lock_generation: AtomicU64::new(0),
        last_inactive_cleanup_unix_ms: AtomicU64::new(0),
        shared_index_generation: AtomicU64::new(shared_index_generation),
        shared_index_store_epoch: AtomicU64::new(shared_index_store_epoch),
        shared_index_change_seq: AtomicU64::new(shared_index_change_seq),
        stats: CacheZoneStats::default(),
        change_notifier: None,
    });
    let manager =
        CacheManager { zones: Arc::new(HashMap::from([("default".to_string(), zone.clone())])) };
    let key = "https:example.com:/local-hot-access";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let old_last_access = now.saturating_sub(5_000);
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        Response::builder()
            .status(StatusCode::OK)
            .header(CACHE_CONTROL, "max-age=60")
            .body(())
            .expect("metadata response should build")
            .headers(),
        test_metadata_input(key, old_last_access, now.saturating_add(60_000), 6),
    );
    let paths = cache_paths_for_zone(zone.config.as_ref(), &hash);
    write_cache_entry(&paths, &metadata, b"cached").await.expect("entry should be written");
    let metadata_sidecar_before_hit =
        tokio::fs::read(&paths.metadata).await.expect("metadata sidecar should be readable");
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            test_index_entry(key, hash.clone(), 6, now.saturating_add(60_000), old_last_access),
        );
        index.current_size_bytes = 6;
    }

    let request = Request::builder()
        .method(Method::GET)
        .uri("/local-hot-access")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    match manager.lookup(CacheRequest::from_request(&request), "https", &test_policy()).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"cached");
        }
        _ => panic!("expected cache hit before cleanup"),
    }

    assert!(
        lock_index(&zone.index)
            .entries
            .get(key)
            .expect("entry should remain indexed after hit")
            .last_access_unix_ms
            >= now,
        "lookup should publish the recent access time to the shared index mirror",
    );
    assert_eq!(
        tokio::fs::read(&paths.metadata).await.expect("metadata sidecar should remain readable"),
        metadata_sidecar_before_hit,
        "lookup should not rewrite the durable metadata sidecar",
    );

    cleanup_inactive_entries_in_zone(&zone).await;

    let index = lock_index(&zone.index);
    let entry = index.entries.get(key).expect("recently hit entry should survive cleanup");
    assert!(
        zone.effective_last_access_unix_ms(key, entry) >= now,
        "local hot access time should override stale durable last_access",
    );
    assert!(paths.metadata.exists());
    assert!(paths.body.exists());
}

#[tokio::test]
async fn hit_reuses_local_response_head_after_metadata_sidecar_is_removed() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let zone = manager.zones.get("default").expect("default zone should exist").clone();
    let key = "https:example.com:/hot-head";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        Response::builder()
            .status(StatusCode::OK)
            .header(CACHE_CONTROL, "max-age=60")
            .body(())
            .expect("metadata response should build")
            .headers(),
        test_metadata_input(key, now.saturating_sub(2_000), now.saturating_add(60_000), 6),
    );
    let paths = cache_paths_for_zone(zone.config.as_ref(), &hash);
    write_cache_entry(&paths, &metadata, b"cached").await.expect("entry should be written");
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(
            key.to_string(),
            test_index_entry(
                key,
                hash.clone(),
                6,
                now.saturating_add(60_000),
                now.saturating_sub(2_000),
            ),
        );
        index.current_size_bytes = 6;
    }

    assert!(zone.prepared_response_head(key, &hash).is_none());

    let request = Request::builder()
        .method(Method::GET)
        .uri("/hot-head")
        .header("host", "example.com")
        .body(full_body(Bytes::new()))
        .expect("request should build");
    match manager.lookup(CacheRequest::from_request(&request), "https", &test_policy()).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"cached");
        }
        _ => panic!("expected first cache hit"),
    }

    assert!(zone.prepared_response_head(key, &hash).is_some());
    tokio::fs::remove_file(&paths.metadata).await.expect("metadata sidecar should be removed");

    match manager.lookup(CacheRequest::from_request(&request), "https", &test_policy()).await {
        CacheLookup::Hit(response) => {
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(body.as_ref(), b"cached");
        }
        _ => panic!("expected hot response head cache hit"),
    }
}

#[tokio::test]
async fn rebucketed_response_still_respects_min_uses_for_new_final_key() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let manager = test_manager(temp.path().to_path_buf(), 1024);
    let zone = manager.zones.get("default").expect("default zone should exist").clone();
    let base_key = "https:example.com:/rebucket";
    let hash = cache_key_hash(base_key);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        base_key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        test_metadata_input(base_key, now.saturating_sub(5_000), now.saturating_sub(1_000), 6),
    );
    let paths = cache_paths_for_zone(zone.config.as_ref(), &hash);
    write_cache_entry(&paths, &metadata, b"cached").await.expect("entry should be written");
    let cached_entry =
        test_index_entry(base_key, hash, 6, now.saturating_sub(1_000), now.saturating_sub(5_000));
    {
        let mut index = lock_index(&zone.index);
        index.insert_entry(base_key.to_string(), cached_entry.clone());
        index.current_size_bytes = 6;
    }

    let expected_final_key = cache_variant_key(
        base_key,
        &[CachedVaryHeaderValue { name: ACCEPT_LANGUAGE, value: Some("zh-CN".to_string()) }],
    );

    for attempt in 1..=2 {
        let mut context = test_store_context(zone.clone(), base_key);
        context.policy.min_uses = 2;
        context.request.uri = http::Uri::from_static("/rebucket");
        context.request.headers.insert(ACCEPT_LANGUAGE, "zh-CN".parse().unwrap());
        context.cached_entry = Some(cached_entry.clone());
        context.key = base_key.to_string();
        context.base_key = base_key.to_string();
        let response = Response::builder()
            .status(StatusCode::OK)
            .header(CACHE_CONTROL, "max-age=60")
            .header("vary", "accept-language")
            .body(full_body("rebucketed"))
            .expect("response should build");
        let _ = drain_response(manager.store_response(context, response).await).await;

        let has_final_key = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let has_final_key =
                    lock_index(&zone.index).entries.contains_key(&expected_final_key);
                if has_final_key == (attempt == 2) {
                    break has_final_key;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("rebucketed cache admission state should settle");
        assert_eq!(
            has_final_key,
            attempt == 2,
            "rebucketed key should only be admitted after min_uses is reached"
        );
    }
}
