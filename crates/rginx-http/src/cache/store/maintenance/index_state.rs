use std::collections::HashMap;
use std::sync::Mutex;

use super::super::super::shared::{
    SharedIndexOperation, apply_zone_shared_index_operations_locked,
};
use super::super::*;
use crate::cache::vary::sorted_vary_dimension_names;

pub(in crate::cache) struct CacheAdmissionDecision {
    pub(in crate::cache) admitted: bool,
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

pub(in crate::cache) async fn remove_zone_index_entry(
    zone: &Arc<CacheZoneRuntime>,
    key: &str,
) -> bool {
    let _sync_guard = zone.shared_index_sync_lock.lock().await;
    let changed = {
        let mut index = lock_index(&zone.index);
        remove_index_entry_locked(&mut index, key)
    };
    if changed {
        apply_zone_shared_index_operations_locked(
            zone.as_ref(),
            &[
                SharedIndexOperation::RemoveEntry { key: key.to_string() },
                SharedIndexOperation::RemoveAdmissionCount { key: key.to_string() },
            ],
        );
        zone.notify_changed();
    }
    changed
}

pub(in crate::cache) fn eviction_candidates(
    index: &mut CacheIndex,
    max_size_bytes: Option<usize>,
) -> Vec<(String, CacheIndexEntry)> {
    let Some(max_size_bytes) = max_size_bytes else {
        return Vec::new();
    };
    if index.current_size_bytes <= max_size_bytes {
        return Vec::new();
    }

    let mut entries =
        index.entries.iter().map(|(key, entry)| (key.clone(), entry.clone())).collect::<Vec<_>>();
    entries.sort_by_key(|(_, entry)| entry.last_access_unix_ms);

    let mut evicted = Vec::new();
    for (key, entry) in entries {
        if index.current_size_bytes <= max_size_bytes {
            break;
        }
        if index.entries.remove(&key).is_some() {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(entry.body_size_bytes);
            index.admission_counts.remove(&key);
            evicted.push((key, entry));
        }
    }
    evicted
}

pub(in crate::cache) fn lock_index(
    mutex: &Mutex<CacheIndex>,
) -> std::sync::MutexGuard<'_, CacheIndex> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
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

fn remove_index_entry_locked(index: &mut CacheIndex, key: &str) -> bool {
    let mut changed = false;
    if let Some(entry) = index.entries.remove(key) {
        index.current_size_bytes = index.current_size_bytes.saturating_sub(entry.body_size_bytes);
        remove_variant_key(&mut index.variants, &entry.base_key, key);
        changed = true;
    }
    changed |= index.admission_counts.remove(key).is_some();
    changed
}
