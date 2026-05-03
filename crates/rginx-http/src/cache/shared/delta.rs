use super::*;

pub(super) fn reload_zone_shared_index(zone: &Arc<CacheZoneRuntime>, store: &SharedIndexStore) {
    let loaded = match run_blocking(|| load_shared_index_from_disk(store)) {
        Ok(loaded) => loaded,
        Err(error) => {
            tracing::warn!(
                zone = %zone.config.name,
                path = %store.path().display(),
                %error,
                "failed to reload shared cache index metadata; keeping local index"
            );
            return;
        }
    };

    let mut index = lock_index(&zone.index);
    *index = loaded.index;
    drop(index);
    zone.clear_hot_entries();
    zone.shared_index_generation.store(loaded.generation, Ordering::Relaxed);
    zone.shared_index_store_epoch.store(loaded.store_epoch, Ordering::Relaxed);
    zone.shared_index_change_seq.store(loaded.last_change_seq, Ordering::Relaxed);
}

pub(super) fn apply_shared_index_delta(
    zone: &CacheZoneRuntime,
    index: &mut CacheIndex,
    operations: &[SharedIndexOperation],
) {
    for operation in operations {
        match operation {
            SharedIndexOperation::UpsertEntry { key, entry } => {
                zone.remove_hot_entry(key);
                if let Some(existing) = index.insert_entry(key.clone(), entry.clone()) {
                    index.current_size_bytes =
                        index.current_size_bytes.saturating_sub(existing.body_size_bytes);
                    remove_variant_key(&mut index.variants, &existing.base_key, key);
                }
                add_variant_key(&mut index.variants, entry.base_key.clone(), key.clone());
                index.current_size_bytes =
                    index.current_size_bytes.saturating_add(entry.body_size_bytes);
            }
            SharedIndexOperation::RemoveEntry { key } => {
                zone.remove_hot_entry(key);
                if let Some(removed) = index.remove_entry(key) {
                    index.current_size_bytes =
                        index.current_size_bytes.saturating_sub(removed.body_size_bytes);
                    remove_variant_key(&mut index.variants, &removed.base_key, key);
                }
            }
            SharedIndexOperation::TouchEntry { key, last_access_unix_ms } => {
                if let Some(entry) = index.entries.get_mut(key)
                    && entry.last_access_unix_ms < *last_access_unix_ms
                {
                    entry.last_access_unix_ms = *last_access_unix_ms;
                    index.reschedule_entry_access(key, *last_access_unix_ms);
                }
            }
            SharedIndexOperation::SetAdmissionCount { key, uses } => {
                index.admission_counts.insert(key.clone(), *uses);
            }
            SharedIndexOperation::RemoveAdmissionCount { key } => {
                index.admission_counts.remove(key);
            }
            SharedIndexOperation::AddInvalidation { rule } => {
                if !index.invalidations.contains(rule) {
                    index.invalidations.push(rule.clone());
                }
                drop_matching_hot_entries(zone, index, rule);
            }
            SharedIndexOperation::ClearInvalidations => {
                index.invalidations.clear();
            }
        }
    }
}

fn drop_matching_hot_entries(
    zone: &CacheZoneRuntime,
    index: &CacheIndex,
    rule: &CacheInvalidationRule,
) {
    for (key, entry) in &index.entries {
        if invalidation_rule_matches_entry(rule, key, entry) {
            zone.remove_hot_entry(key);
        }
    }
}

fn add_variant_key(
    variants: &mut std::collections::HashMap<String, Vec<String>>,
    base_key: String,
    key: String,
) {
    let keys = variants.entry(base_key).or_default();
    if !keys.contains(&key) {
        keys.push(key);
    }
}

fn remove_variant_key(
    variants: &mut std::collections::HashMap<String, Vec<String>>,
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
