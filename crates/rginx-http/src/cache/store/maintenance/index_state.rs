use std::collections::HashMap;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

use super::super::super::shared::{
    SharedIndexOperation, apply_zone_shared_index_operations_locked,
};
use super::super::*;
use crate::cache::vary::sorted_vary_dimension_names;

pub(in crate::cache) struct CacheAdmissionDecision {
    pub(in crate::cache) admitted: bool,
}

pub(in crate::cache) struct RemovedIndexEntry {
    pub(in crate::cache) hash: String,
    pub(in crate::cache) delete_files: bool,
}

pub(in crate::cache) async fn record_cache_admission_attempt(
    zone: &Arc<CacheZoneRuntime>,
    key: &str,
    min_uses: u64,
) -> CacheAdmissionDecision {
    let _sync_guard = zone.shared_index_sync_lock.lock().await;
    let (admitted, shared_operations) = {
        let mut index = lock_index(&zone.index);
        if min_uses <= 1 || index.entries.contains_key(key) {
            let shared_operations = index
                .admission_counts
                .remove(key)
                .map(|_| SharedIndexOperation::RemoveAdmissionCount { key: key.to_string() })
                .into_iter()
                .collect();
            (true, shared_operations)
        } else {
            let uses = index.admission_counts.entry(key.to_string()).or_insert(0);
            *uses = uses.saturating_add(1);
            if *uses >= min_uses {
                index.admission_counts.remove(key);
                (true, vec![SharedIndexOperation::RemoveAdmissionCount { key: key.to_string() }])
            } else {
                (
                    false,
                    vec![SharedIndexOperation::SetAdmissionCount {
                        key: key.to_string(),
                        uses: *uses,
                    }],
                )
            }
        }
    };
    apply_zone_shared_index_operations_locked(zone.as_ref(), &shared_operations);
    CacheAdmissionDecision { admitted }
}

pub(in crate::cache) async fn remove_zone_index_entry_if_matches(
    zone: &Arc<CacheZoneRuntime>,
    key: &str,
    expected_entry: &CacheIndexEntry,
) -> Option<RemovedIndexEntry> {
    let _sync_guard = zone.shared_index_sync_lock.lock().await;
    let removed = {
        let mut index = lock_index(&zone.index);
        let current = index.entries.get(key)?;
        if !current.stable_eq(expected_entry) {
            return None;
        }
        let removed = index.remove_entry(key).expect("matching cache key should still exist");
        index.current_size_bytes = index.current_size_bytes.saturating_sub(removed.body_size_bytes);
        index.admission_counts.remove(key);
        remove_variant_key(&mut index.variants, &removed.base_key, key);
        let delete_files = !index.hash_is_referenced(&removed.hash);
        RemovedIndexEntry { hash: removed.hash, delete_files }
    };
    apply_zone_shared_index_operations_locked(
        zone.as_ref(),
        &[
            SharedIndexOperation::RemoveEntry { key: key.to_string() },
            SharedIndexOperation::RemoveAdmissionCount { key: key.to_string() },
        ],
    );
    zone.remove_hot_entry(key);
    zone.notify_changed();
    Some(removed)
}

pub(in crate::cache) fn eviction_candidates(
    zone: &CacheZoneRuntime,
    index: &mut CacheIndex,
    max_size_bytes: Option<usize>,
) -> Vec<(String, CacheIndexEntry)> {
    let Some(max_size_bytes) = max_size_bytes else {
        return Vec::new();
    };
    if index.current_size_bytes <= max_size_bytes {
        return Vec::new();
    }

    let mut evicted = Vec::new();
    while index.current_size_bytes > max_size_bytes {
        let Some((key, scheduled_last_access_unix_ms)) = index.pop_oldest_scheduled_entry() else {
            break;
        };
        let Some(entry) = index.entries.get(&key).cloned() else {
            continue;
        };
        let effective_last_access_unix_ms = zone.effective_last_access_unix_ms(&key, &entry);
        if effective_last_access_unix_ms > scheduled_last_access_unix_ms {
            index.reschedule_entry_access(&key, effective_last_access_unix_ms);
            continue;
        }

        if let Some(entry) = index.remove_entry(&key) {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(entry.body_size_bytes);
            index.admission_counts.remove(&key);
            evicted.push((key, entry));
        }
    }
    evicted
}

pub(in crate::cache) fn inactive_cleanup_candidates(
    zone: &CacheZoneRuntime,
    index: &mut CacheIndex,
    now: u64,
    inactive_ms: u64,
    batch_size: usize,
) -> (Vec<(String, CacheIndexEntry)>, bool) {
    if batch_size == 0 {
        return (Vec::new(), false);
    }

    let inactive_cutoff_last_access_unix_ms = now.saturating_sub(inactive_ms);
    let mut removed = Vec::new();
    while removed.len() < batch_size {
        let Some(oldest_last_access_unix_ms) = index.oldest_scheduled_access_unix_ms() else {
            break;
        };
        if oldest_last_access_unix_ms >= inactive_cutoff_last_access_unix_ms {
            break;
        }

        let Some((key, scheduled_last_access_unix_ms)) = index.pop_oldest_scheduled_entry() else {
            break;
        };
        let Some(entry) = index.entries.get(&key).cloned() else {
            continue;
        };
        let effective_last_access_unix_ms = zone.effective_last_access_unix_ms(&key, &entry);
        if effective_last_access_unix_ms > scheduled_last_access_unix_ms {
            index.reschedule_entry_access(&key, effective_last_access_unix_ms);
            continue;
        }
        if now.saturating_sub(effective_last_access_unix_ms) <= inactive_ms {
            index.reschedule_entry_access(&key, effective_last_access_unix_ms);
            continue;
        }

        if let Some(entry) = index.remove_entry(&key) {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(entry.body_size_bytes);
            index.admission_counts.remove(&key);
            removed.push((key, entry));
        }
    }

    let has_more_due = index.oldest_scheduled_access_unix_ms().is_some_and(|last_access_unix_ms| {
        last_access_unix_ms < inactive_cutoff_last_access_unix_ms
    });
    (removed, has_more_due)
}

pub(in crate::cache) fn read_index(index: &RwLock<CacheIndex>) -> RwLockReadGuard<'_, CacheIndex> {
    index.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(in crate::cache) fn lock_index(index: &RwLock<CacheIndex>) -> RwLockWriteGuard<'_, CacheIndex> {
    index.write().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(super) fn add_variant_key(
    variants: &mut HashMap<String, Vec<String>>,
    base_key: String,
    key: String,
) {
    let entry = variants.entry(base_key).or_default();
    if !entry.contains(&key) {
        entry.push(key);
    }
}

pub(super) fn remove_variant_key(
    variants: &mut HashMap<String, Vec<String>>,
    base_key: &str,
    key: &str,
) {
    let Some(keys) = variants.get_mut(base_key) else {
        return;
    };
    keys.retain(|candidate| candidate != key);
    if keys.is_empty() {
        variants.remove(base_key);
    }
}

pub(super) fn variant_keys_with_different_dimensions(
    index: &CacheIndex,
    entry: &CacheIndexEntry,
    key: &str,
) -> Vec<String> {
    let expected_names = sorted_vary_dimension_names(&entry.vary);
    index
        .variants
        .get(&entry.base_key)
        .into_iter()
        .flatten()
        .filter(|candidate| candidate.as_str() != key)
        .filter_map(|candidate| {
            let existing = index.entries.get(candidate)?;
            let existing_names = sorted_vary_dimension_names(&existing.vary);
            (existing_names != expected_names).then_some(candidate.clone())
        })
        .collect()
}
