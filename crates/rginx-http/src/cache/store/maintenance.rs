use super::*;
use crate::cache::PurgeSelector;
use crate::cache::vary::sorted_vary_dimension_names;
use std::collections::HashMap;

pub(in crate::cache) async fn cleanup_inactive_entries_in_zone(zone: &Arc<CacheZoneRuntime>) {
    let now = unix_time_ms(SystemTime::now());
    let interval_ms = duration_to_ms(zone.config.inactive_cleanup_interval);
    let last_cleanup =
        zone.last_inactive_cleanup_unix_ms.load(std::sync::atomic::Ordering::Relaxed);
    if last_cleanup != 0 && now.saturating_sub(last_cleanup) < interval_ms {
        return;
    }
    zone.last_inactive_cleanup_unix_ms.store(now, std::sync::atomic::Ordering::Relaxed);

    let inactive_ms = duration_to_ms(zone.config.inactive);
    let batch_size = zone.config.manager_batch_entries.max(1);
    let mut total_removed = 0usize;
    let mut changed = false;

    loop {
        let removed = {
            let mut index = lock_index(&zone.index);
            let mut keys_to_remove = index
                .entries
                .iter()
                .filter_map(|(key, entry)| {
                    (now.saturating_sub(entry.last_access_unix_ms) > inactive_ms)
                        .then_some((key.clone(), entry.last_access_unix_ms))
                })
                .collect::<Vec<_>>();
            keys_to_remove.sort_by_key(|(_, last_access)| *last_access);
            let keys_to_remove =
                keys_to_remove.into_iter().take(batch_size).map(|(key, _)| key).collect::<Vec<_>>();
            let mut removed = Vec::new();
            for key in keys_to_remove {
                if let Some(entry) = index.entries.remove(&key) {
                    index.current_size_bytes =
                        index.current_size_bytes.saturating_sub(entry.body_size_bytes);
                    index.admission_counts.remove(&key);
                    remove_variant_key(&mut index.variants, &entry.base_key, &key);
                    removed.push(entry);
                }
            }
            removed
        };
        if removed.is_empty() {
            break;
        }
        changed = true;
        total_removed += removed.len();
        let io_guard = zone.io_lock.lock().await;
        for entry in &removed {
            let paths = cache_paths_for_zone(zone.config.as_ref(), &entry.hash);
            let _ = fs::remove_file(paths.metadata).await;
            let _ = fs::remove_file(paths.body).await;
        }
        drop(io_guard);
        if removed.len() < batch_size {
            break;
        }
        let has_more_inactive = {
            let index = lock_index(&zone.index);
            index
                .entries
                .values()
                .any(|entry| now.saturating_sub(entry.last_access_unix_ms) > inactive_ms)
        };
        if !has_more_inactive {
            break;
        }
        if !zone.config.manager_sleep.is_zero() {
            tokio::time::sleep(zone.config.manager_sleep).await;
        }
    }

    if changed {
        zone.record_inactive_cleanup(total_removed);
        zone.notify_changed();
    }
}

pub(in crate::cache) async fn purge_zone_entries(
    zone: Arc<CacheZoneRuntime>,
    selector: PurgeSelector,
) -> CachePurgeResult {
    let scope = purge_scope(&selector);
    let removed = {
        let mut index = lock_index(&zone.index);
        let matching_keys = index
            .entries
            .keys()
            .filter_map(|key| purge_selector_matches(&selector, key).then_some(key.clone()))
            .collect::<Vec<_>>();
        let mut removed = Vec::with_capacity(matching_keys.len());
        for key in matching_keys {
            if let Some(entry) = index.entries.remove(&key) {
                index.current_size_bytes =
                    index.current_size_bytes.saturating_sub(entry.body_size_bytes);
                index.admission_counts.remove(&key);
                remove_variant_key(&mut index.variants, &entry.base_key, &key);
                removed.push(entry);
            }
        }
        removed
    };
    let removed_bytes = removed.iter().map(|entry| entry.body_size_bytes).sum::<usize>();
    if !removed.is_empty() {
        zone.record_purge(removed.len());
        let _io_guard = zone.io_lock.lock().await;
        for entry in &removed {
            let paths = cache_paths_for_zone(zone.config.as_ref(), &entry.hash);
            let _ = fs::remove_file(paths.metadata).await;
            let _ = fs::remove_file(paths.body).await;
        }
        zone.notify_changed();
    }
    CachePurgeResult {
        zone_name: zone.config.name.clone(),
        scope,
        removed_entries: removed.len(),
        removed_bytes,
    }
}

pub(in crate::cache) async fn update_index_after_store(
    zone: &Arc<CacheZoneRuntime>,
    key: String,
    entry: CacheIndexEntry,
    replaced_entry: Option<(String, CacheIndexEntry)>,
) {
    let (removed_hashes, eviction_count) = {
        let mut index = lock_index(&zone.index);
        let mut removed_hashes = Vec::new();
        let mut eviction_count = 0usize;
        if let Some((replaced_key, _)) = replaced_entry
            && let Some(removed) = index.entries.remove(&replaced_key)
        {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(removed.body_size_bytes);
            index.admission_counts.remove(&replaced_key);
            remove_variant_key(&mut index.variants, &removed.base_key, &replaced_key);
            if removed.hash != entry.hash {
                removed_hashes.push(removed.hash);
            }
        }
        let incompatible_keys = variant_keys_with_different_dimensions(&index, &entry, &key);
        for incompatible_key in incompatible_keys {
            if let Some(removed) = index.entries.remove(&incompatible_key) {
                index.current_size_bytes =
                    index.current_size_bytes.saturating_sub(removed.body_size_bytes);
                index.admission_counts.remove(&incompatible_key);
                remove_variant_key(&mut index.variants, &removed.base_key, &incompatible_key);
                if removed.hash != entry.hash {
                    removed_hashes.push(removed.hash);
                }
            }
        }

        if let Some(existing) = index.entries.insert(key.clone(), entry.clone()) {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(existing.body_size_bytes);
            index.admission_counts.remove(&key);
            remove_variant_key(&mut index.variants, &existing.base_key, &key);
            if existing.hash != entry.hash {
                removed_hashes.push(existing.hash);
            }
        }
        index.admission_counts.remove(&key);
        add_variant_key(&mut index.variants, entry.base_key.clone(), key);
        index.current_size_bytes = index.current_size_bytes.saturating_add(entry.body_size_bytes);
        for (evicted_key, evicted_entry) in
            eviction_candidates(&mut index, zone.config.max_size_bytes)
        {
            index.admission_counts.remove(&evicted_key);
            remove_variant_key(&mut index.variants, &evicted_entry.base_key, &evicted_key);
            if evicted_entry.hash != entry.hash {
                removed_hashes.push(evicted_entry.hash);
            }
            eviction_count += 1;
        }
        (removed_hashes, eviction_count)
    };

    if eviction_count > 0 {
        zone.record_evictions(eviction_count);
    }
    for hash in removed_hashes {
        let paths = cache_paths_for_zone(zone.config.as_ref(), &hash);
        let _ = fs::remove_file(paths.metadata).await;
        let _ = fs::remove_file(paths.body).await;
    }
    zone.notify_changed();
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

pub(in crate::cache) fn remove_index_entry(zone: &CacheZoneRuntime, key: &str) {
    let mut index = lock_index(&zone.index);
    if let Some(entry) = index.entries.remove(key) {
        index.current_size_bytes = index.current_size_bytes.saturating_sub(entry.body_size_bytes);
        remove_variant_key(&mut index.variants, &entry.base_key, key);
    }
    index.admission_counts.remove(key);
    zone.notify_changed();
}

pub(in crate::cache) fn lock_index(
    mutex: &Mutex<CacheIndex>,
) -> std::sync::MutexGuard<'_, CacheIndex> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(in crate::cache) fn record_cache_admission_attempt(
    zone: &CacheZoneRuntime,
    key: &str,
    min_uses: u64,
    cached_entry_exists: bool,
) -> bool {
    if cached_entry_exists || min_uses <= 1 {
        let mut index = lock_index(&zone.index);
        index.admission_counts.remove(key);
        return true;
    }

    let mut index = lock_index(&zone.index);
    let uses = index.admission_counts.entry(key.to_string()).or_insert(0);
    *uses = uses.saturating_add(1);
    if *uses >= min_uses {
        index.admission_counts.remove(key);
        true
    } else {
        false
    }
}

fn add_variant_key(variants: &mut HashMap<String, Vec<String>>, base_key: String, key: String) {
    let entry = variants.entry(base_key).or_default();
    if !entry.contains(&key) {
        entry.push(key);
    }
}

fn remove_variant_key(variants: &mut HashMap<String, Vec<String>>, base_key: &str, key: &str) {
    let Some(keys) = variants.get_mut(base_key) else {
        return;
    };
    keys.retain(|candidate| candidate != key);
    if keys.is_empty() {
        variants.remove(base_key);
    }
}

fn variant_keys_with_different_dimensions(
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
