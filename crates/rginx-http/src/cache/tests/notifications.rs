use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

use http::StatusCode;

use super::*;

#[test]
fn counter_updates_do_not_notify_change_listeners() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let notifications = Arc::new(AtomicUsize::new(0));
    let notifier = {
        let notifications = notifications.clone();
        Arc::new(move |_zone_name: &str| {
            notifications.fetch_add(1, Ordering::Relaxed);
        })
    };
    let zone = test_zone_with_notifier(temp.path().to_path_buf(), 1024, Some(notifier));

    zone.record_hit();
    zone.record_miss();
    zone.record_bypass();
    zone.record_expired();
    zone.record_stale();
    zone.record_revalidated();
    zone.record_write_success();
    zone.record_write_error();
    zone.record_evictions(1);
    zone.record_purge(1);
    zone.record_inactive_cleanup(1);

    assert_eq!(notifications.load(Ordering::Relaxed), 0);
}

#[tokio::test]
async fn cleanup_inactive_entries_notifies_change_listeners() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let notifications = Arc::new(AtomicUsize::new(0));
    let notifier = {
        let notifications = notifications.clone();
        Arc::new(move |_zone_name: &str| {
            notifications.fetch_add(1, Ordering::Relaxed);
        })
    };
    let zone = test_zone_with_notifier(temp.path().to_path_buf(), 1024, Some(notifier));
    let key = "https:example.com:/inactive";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        test_metadata_input(now.saturating_sub(120_000), now.saturating_add(60_000), 6),
    );
    let paths = cache_paths(temp.path(), &hash);
    write_cache_entry(&paths, &metadata, b"cached").await.expect("entry should be written");
    {
        let mut index = lock_index(&zone.index);
        index.entries.insert(
            key.to_string(),
            test_index_entry(hash, 6, now.saturating_add(60_000), now.saturating_sub(120_000)),
        );
        index.current_size_bytes = 6;
    }

    cleanup_inactive_entries_in_zone(&zone).await;

    assert_eq!(notifications.load(Ordering::Relaxed), 1);
    assert!(!lock_index(&zone.index).entries.contains_key(key));
    assert!(!paths.metadata.exists());
    assert!(!paths.body.exists());
}

#[tokio::test]
async fn purge_zone_entries_notifies_change_listeners() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let notifications = Arc::new(AtomicUsize::new(0));
    let notifier = {
        let notifications = notifications.clone();
        Arc::new(move |_zone_name: &str| {
            notifications.fetch_add(1, Ordering::Relaxed);
        })
    };
    let zone = test_zone_with_notifier(temp.path().to_path_buf(), 1024, Some(notifier));
    let key = "https:example.com:/purge";
    let hash = cache_key_hash(key);
    let now = unix_time_ms(SystemTime::now());
    let metadata = cache_metadata(
        key.to_string(),
        StatusCode::OK,
        &http::HeaderMap::new(),
        test_metadata_input(now.saturating_sub(2_000), now.saturating_add(60_000), 6),
    );
    let paths = cache_paths(temp.path(), &hash);
    write_cache_entry(&paths, &metadata, b"cached").await.expect("entry should be written");
    {
        let mut index = lock_index(&zone.index);
        index.entries.insert(
            key.to_string(),
            test_index_entry(hash, 6, now.saturating_add(60_000), now.saturating_sub(1_000)),
        );
        index.current_size_bytes = 6;
    }

    let result = purge_zone_entries(zone.clone(), PurgeSelector::All).await;

    assert_eq!(result.removed_entries, 1);
    assert_eq!(notifications.load(Ordering::Relaxed), 1);
    assert!(!lock_index(&zone.index).entries.contains_key(key));
    assert!(!paths.metadata.exists());
    assert!(!paths.body.exists());
}
