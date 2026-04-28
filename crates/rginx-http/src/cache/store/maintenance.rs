use super::*;

pub(in crate::cache) async fn cleanup_inactive_entries_in_zone(zone: &Arc<CacheZoneRuntime>) {
    let inactive_ms = duration_to_ms(zone.config.inactive);
    let now = unix_time_ms(SystemTime::now());
    let removed = {
        let mut index = lock_index(&zone.index);
        let keys_to_remove = index
            .entries
            .iter()
            .filter_map(|(key, entry)| {
                (now.saturating_sub(entry.last_access_unix_ms) > inactive_ms).then_some(key.clone())
            })
            .collect::<Vec<_>>();
        let mut removed = Vec::new();
        for key in keys_to_remove {
            if let Some(entry) = index.entries.remove(&key) {
                index.current_size_bytes =
                    index.current_size_bytes.saturating_sub(entry.body_size_bytes);
                removed.push(entry);
            }
        }
        removed
    };
    if removed.is_empty() {
        return;
    }
    zone.record_inactive_cleanup(removed.len());
    let _io_guard = zone.io_lock.lock().await;
    for entry in &removed {
        let paths = cache_paths(&zone.config.path, &entry.hash);
        let _ = fs::remove_file(paths.metadata).await;
        let _ = fs::remove_file(paths.body).await;
    }
    zone.notify_changed();
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
            let paths = cache_paths(&zone.config.path, &entry.hash);
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
) {
    let evictions = {
        let mut index = lock_index(&zone.index);
        if let Some(existing) = index.entries.insert(key, entry.clone()) {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(existing.body_size_bytes);
        }
        index.current_size_bytes = index.current_size_bytes.saturating_add(entry.body_size_bytes);
        eviction_candidates(&mut index, zone.config.max_size_bytes)
    };

    if !evictions.is_empty() {
        zone.record_evictions(evictions.len());
    }
    for hash in evictions {
        let paths = cache_paths(&zone.config.path, &hash);
        let _ = fs::remove_file(paths.metadata).await;
        let _ = fs::remove_file(paths.body).await;
    }
    zone.notify_changed();
}

pub(in crate::cache) fn eviction_candidates(
    index: &mut CacheIndex,
    max_size_bytes: Option<usize>,
) -> Vec<String> {
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
            evicted.push(entry.hash);
        }
    }
    evicted
}

pub(in crate::cache) fn remove_index_entry(zone: &CacheZoneRuntime, key: &str) {
    let mut index = lock_index(&zone.index);
    if let Some(entry) = index.entries.remove(key) {
        index.current_size_bytes = index.current_size_bytes.saturating_sub(entry.body_size_bytes);
    }
    zone.notify_changed();
}

pub(in crate::cache) fn lock_index(
    mutex: &Mutex<CacheIndex>,
) -> std::sync::MutexGuard<'_, CacheIndex> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
