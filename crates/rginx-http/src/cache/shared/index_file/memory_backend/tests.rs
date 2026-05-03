use std::io;
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::super::{SharedIndexBackend, SharedIndexOperation};
use super::MemorySharedIndexStore;
use crate::cache::shared::memory::{SharedMemorySegment, SharedMemorySegmentConfig};
use crate::cache::{
    CacheIndex, CacheIndexEntry, CacheIndexEntryKind, CacheInvalidationRule,
    CacheInvalidationSelector,
};

pub(super) fn unlink_for_zone(zone: &rginx_core::CacheZone) -> io::Result<()> {
    let identity = format!("{}:{}", zone.name, zone.path.display());
    let segment_config =
        SharedMemorySegmentConfig::for_identity(&identity, super::memory_capacity_bytes());
    SharedMemorySegment::unlink(&segment_config)
}

pub(super) fn corrupt_header_for_zone(zone: &rginx_core::CacheZone) -> io::Result<()> {
    let store = MemorySharedIndexStore::new(zone);
    let _lock = store.lock()?;
    let segment = store.open_or_create_segment()?;
    let mut header = segment.header();
    header.abi_version = header.abi_version.saturating_add(1);
    segment.write_header(header);
    Ok(())
}

pub(super) fn corrupt_document_for_zone(zone: &rginx_core::CacheZone) -> io::Result<()> {
    let store = MemorySharedIndexStore::new(zone);
    let _lock = store.lock()?;
    let segment = store.open_or_create_segment()?;
    let invalid_len =
        u64::try_from(segment.payload_capacity()).unwrap_or(u64::MAX).saturating_add(1);
    segment.write_payload(0, &invalid_len.to_le_bytes())?;
    Ok(())
}

#[test]
fn memory_backend_shares_snapshot_between_store_instances() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path());
    let store_a = MemorySharedIndexStore::new(&zone);
    let store_b = MemorySharedIndexStore::new(&zone);
    let _ = SharedMemorySegment::unlink(&store_a.segment_config);
    let mut index = CacheIndex::default();
    index.insert_entry("https:example.com:/shm".to_string(), test_entry("/shm"));
    index.admission_counts.insert("https:example.com:/shm".to_string(), 3);

    let applied = store_a.recreate(&index, 11).expect("shm recreate should succeed");
    assert_eq!(applied.generation, 11);
    assert_eq!(applied.last_change_seq, 0);

    let loaded = store_b.load().expect("second shm store should load snapshot");
    assert_eq!(loaded.generation, 11);
    assert_eq!(loaded.index.admission_counts.get("https:example.com:/shm"), Some(&3));
    assert!(loaded.index.entries.contains_key("https:example.com:/shm"));
    let _ = SharedMemorySegment::unlink(&store_a.segment_config);
}

#[test]
fn memory_backend_replays_bounded_changes_and_falls_back_when_gap_exists() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path());
    let mut store = MemorySharedIndexStore::new(&zone);
    store.operation_ring_capacity = 1;
    store.segment_config = store.segment_config.clone().with_operation_ring_capacity(1);
    let _ = SharedMemorySegment::unlink(&store.segment_config);

    store.recreate(&CacheIndex::default(), 1).expect("shm recreate should succeed");
    store
        .apply_operations(&[SharedIndexOperation::UpsertEntry {
            key: "https:example.com:/one".to_string(),
            entry: test_entry("/one"),
        }])
        .expect("first shm operation should apply");
    store
        .apply_operations(&[SharedIndexOperation::SetAdmissionCount {
            key: "https:example.com:/one".to_string(),
            uses: 2,
        }])
        .expect("second shm operation should apply");

    let replay = store.load_changes_since(1).expect("latest change should replay");
    assert_eq!(replay.operations.len(), 1);
    assert!(matches!(replay.operations[0], SharedIndexOperation::SetAdmissionCount { .. }));

    let gap = store.load_changes_since(0).expect("gap should request full reload");
    assert!(gap.operations.is_empty());
    assert_eq!(gap.last_change_seq, 2);

    let loaded = store.load().expect("full reload should remain available");
    assert!(loaded.index.entries.contains_key("https:example.com:/one"));
    assert_eq!(loaded.index.admission_counts.get("https:example.com:/one"), Some(&2));
    let _ = SharedMemorySegment::unlink(&store.segment_config);
}

#[test]
fn memory_backend_persists_invalidations_and_replays_delta() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path());
    let store = MemorySharedIndexStore::new(&zone);
    let _ = SharedMemorySegment::unlink(&store.segment_config);

    store.recreate(&CacheIndex::default(), 1).expect("shm recreate should succeed");
    let rule = CacheInvalidationRule {
        selector: CacheInvalidationSelector::Exact("https:example.com:/invalidate".to_string()),
        created_at_unix_ms: 1_000,
    };
    store
        .apply_operations(&[SharedIndexOperation::AddInvalidation { rule: rule.clone() }])
        .expect("invalidation should apply");

    let loaded = store.load().expect("invalidations should load from shm");
    assert_eq!(loaded.index.invalidations, vec![rule.clone()]);

    let replay = store.load_changes_since(0).expect("invalidation delta should replay");
    assert_eq!(replay.operations.len(), 1);
    assert!(matches!(replay.operations[0], SharedIndexOperation::AddInvalidation { .. }));
    let _ = SharedMemorySegment::unlink(&store.segment_config);
}

#[test]
fn memory_backend_metrics_track_reloads_contention_and_ring_usage() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path());
    let mut store = Arc::new(MemorySharedIndexStore::new(&zone));
    {
        let store_mut = Arc::get_mut(&mut store).expect("store should be uniquely owned");
        store_mut.operation_ring_capacity = 1;
        store_mut.segment_config = store_mut.segment_config.clone().with_operation_ring_capacity(1);
    }
    let _ = SharedMemorySegment::unlink(&store.segment_config);

    store.recreate(&CacheIndex::default(), 1).expect("shm recreate should succeed");
    store
        .apply_operations(&[SharedIndexOperation::UpsertEntry {
            key: "https:example.com:/metrics".to_string(),
            entry: test_entry("/metrics"),
        }])
        .expect("upsert should succeed");
    let loaded = store.load().expect("full reload should succeed");
    assert!(loaded.index.entries.contains_key("https:example.com:/metrics"));

    let (lock_acquired_tx, lock_acquired_rx) = mpsc::channel();
    let holder = {
        let store = store.clone();
        thread::spawn(move || {
            store
                .with_document_lock(|_, _| {
                    lock_acquired_tx.send(()).expect("lock acquisition should signal");
                    thread::sleep(Duration::from_millis(50));
                    Ok(())
                })
                .expect("lock holder should succeed");
        })
    };
    lock_acquired_rx.recv().expect("lock holder should acquire lock");
    let metrics = store.metrics().expect("metrics should load");
    holder.join().expect("lock holder should join");

    assert_eq!(metrics.rebuild_total, 1);
    assert_eq!(metrics.full_reload_total, 1);
    assert_eq!(metrics.operation_ring_capacity, 1);
    assert_eq!(metrics.operation_ring_used, 1);
    assert!(metrics.shm_used_bytes > 0);
    assert!(metrics.lock_contention_total >= 1);
    let _ = SharedMemorySegment::unlink(&store.segment_config);
}

#[test]
fn memory_backend_counts_capacity_rejections_for_oversized_documents() {
    let temp = tempfile::tempdir().expect("cache temp dir should exist");
    let zone = test_zone(temp.path());
    let mut store = MemorySharedIndexStore::new(&zone);
    store.segment_config.capacity_bytes = 1_024;
    store.segment_config = store.segment_config.clone().with_operation_ring_capacity(1);
    let _ = SharedMemorySegment::unlink(&store.segment_config);

    store.recreate(&CacheIndex::default(), 1).expect("empty shm recreate should succeed");
    let oversized_path = format!("/{}", "x".repeat(4_096));
    let result = store.apply_operations(&[SharedIndexOperation::UpsertEntry {
        key: format!("https:example.com:{oversized_path}"),
        entry: test_entry(&oversized_path),
    }]);
    let error = match result {
        Ok(_) => panic!("oversized document should be rejected"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), io::ErrorKind::OutOfMemory);

    let metrics = store.metrics().expect("metrics should load");
    assert_eq!(metrics.capacity_rejection_total, 1);
    assert_eq!(metrics.rebuild_total, 1);
    let _ = SharedMemorySegment::unlink(&store.segment_config);
}

fn test_zone(path: &Path) -> rginx_core::CacheZone {
    rginx_core::CacheZone {
        name: format!("shm-test-{}", unique_suffix()),
        path: path.to_path_buf(),
        max_size_bytes: Some(1024 * 1024),
        inactive: std::time::Duration::from_secs(60),
        default_ttl: std::time::Duration::from_secs(60),
        max_entry_bytes: 1024,
        path_levels: vec![2],
        loader_batch_entries: 100,
        loader_sleep: std::time::Duration::ZERO,
        manager_batch_entries: 100,
        manager_sleep: std::time::Duration::ZERO,
        inactive_cleanup_interval: std::time::Duration::from_secs(60),
        shared_index: true,
    }
}

fn test_entry(path: &str) -> CacheIndexEntry {
    CacheIndexEntry {
        kind: CacheIndexEntryKind::Response,
        hash: format!("hash-{path}"),
        base_key: format!("https:example.com:{path}"),
        stored_at_unix_ms: 1_000,
        vary: Vec::new(),
        tags: Vec::new(),
        body_size_bytes: 3,
        expires_at_unix_ms: 60_000,
        grace_until_unix_ms: None,
        keep_until_unix_ms: None,
        stale_if_error_until_unix_ms: None,
        stale_while_revalidate_until_unix_ms: None,
        requires_revalidation: false,
        must_revalidate: false,
        last_access_unix_ms: 1_000,
    }
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos()
}
