use super::*;

pub(in crate::cache) async fn update_index_after_store(
    zone: &Arc<CacheZoneRuntime>,
    key: String,
    entry: CacheIndexEntry,
    replaced_entry: Option<(String, CacheIndexEntry)>,
) -> std::collections::BTreeSet<String> {
    let (removed_hashes, removed_keys, eviction_count) = {
        let _sync_guard = zone.shared_index_sync_lock.lock().await;
        let mut index = lock_index(&zone.index);
        let mut removed_hashes = std::collections::BTreeSet::new();
        let mut removed_keys = Vec::new();
        let mut eviction_count = 0usize;
        let mut shared_operations = Vec::new();

        if let Some((replaced_key, expected_entry)) = replaced_entry
            && index
                .entries
                .get(&replaced_key)
                .is_some_and(|current| current.stable_eq(&expected_entry))
            && let Some(removed) = index.remove_entry(&replaced_key)
        {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(removed.body_size_bytes);
            index.admission_counts.remove(&replaced_key);
            remove_variant_key(&mut index.variants, &removed.base_key, &replaced_key);
            if removed.hash != entry.hash {
                removed_hashes.insert(removed.hash);
            }
            removed_keys.push(replaced_key.clone());
            shared_operations.push(SharedIndexOperation::RemoveEntry { key: replaced_key.clone() });
            shared_operations
                .push(SharedIndexOperation::RemoveAdmissionCount { key: replaced_key });
        }

        let incompatible_keys = variant_keys_with_different_dimensions(&index, &entry, &key);
        for incompatible_key in incompatible_keys {
            if let Some(removed) = index.remove_entry(&incompatible_key) {
                index.current_size_bytes =
                    index.current_size_bytes.saturating_sub(removed.body_size_bytes);
                index.admission_counts.remove(&incompatible_key);
                remove_variant_key(&mut index.variants, &removed.base_key, &incompatible_key);
                if removed.hash != entry.hash {
                    removed_hashes.insert(removed.hash);
                }
                removed_keys.push(incompatible_key.clone());
                shared_operations
                    .push(SharedIndexOperation::RemoveEntry { key: incompatible_key.clone() });
                shared_operations
                    .push(SharedIndexOperation::RemoveAdmissionCount { key: incompatible_key });
            }
        }

        if let Some(existing) = index.insert_entry(key.clone(), entry.clone()) {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(existing.body_size_bytes);
            remove_variant_key(&mut index.variants, &existing.base_key, &key);
            if existing.hash != entry.hash {
                removed_hashes.insert(existing.hash);
            }
            removed_keys.push(key.clone());
        }
        index.admission_counts.remove(&key);
        add_variant_key(&mut index.variants, entry.base_key.clone(), key.clone());
        index.current_size_bytes = index.current_size_bytes.saturating_add(entry.body_size_bytes);
        shared_operations
            .push(SharedIndexOperation::UpsertEntry { key: key.clone(), entry: entry.clone() });
        shared_operations.push(SharedIndexOperation::RemoveAdmissionCount { key: key.clone() });

        for (evicted_key, evicted_entry) in
            eviction_candidates(zone.as_ref(), &mut index, zone.config.max_size_bytes)
        {
            index.admission_counts.remove(&evicted_key);
            remove_variant_key(&mut index.variants, &evicted_entry.base_key, &evicted_key);
            if evicted_entry.hash != entry.hash {
                removed_hashes.insert(evicted_entry.hash);
            }
            removed_keys.push(evicted_key.clone());
            eviction_count += 1;
            shared_operations.push(SharedIndexOperation::RemoveEntry { key: evicted_key.clone() });
            shared_operations.push(SharedIndexOperation::RemoveAdmissionCount { key: evicted_key });
        }

        apply_zone_shared_index_operations_locked(zone.as_ref(), &shared_operations);
        (removed_hashes, removed_keys, eviction_count)
    };

    for removed_key in removed_keys {
        zone.remove_hot_entry(&removed_key);
    }
    if eviction_count > 0 {
        zone.record_evictions(eviction_count);
    }
    zone.notify_changed();
    removed_hashes
}
