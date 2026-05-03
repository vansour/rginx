use super::super::invalidation::{
    entry_is_logically_invalid, invalidation_rule_matches_entry, invalidation_scope,
};
use super::super::remove_cache_files_if_unreferenced;
use super::super::shared::{SharedIndexOperation, apply_zone_shared_index_operations_locked};
use super::*;
use crate::cache::{
    CacheInvalidationResult, CacheInvalidationRule, CacheInvalidationSelector, PurgeSelector,
};

mod index_state;
mod store_update;

use index_state::{add_variant_key, remove_variant_key, variant_keys_with_different_dimensions};
pub(in crate::cache) use index_state::{
    eviction_candidates, inactive_cleanup_candidates, lock_index, read_index,
    record_cache_admission_attempt, remove_zone_index_entry_if_matches,
};
pub(in crate::cache) use store_update::update_index_after_store;

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
        let (removed, has_more_due) = {
            let _sync_guard = zone.shared_index_sync_lock.lock().await;
            let mut index = lock_index(&zone.index);
            let (removed, has_more_due) = inactive_cleanup_candidates(
                zone.as_ref(),
                &mut index,
                now,
                inactive_ms,
                batch_size,
            );
            let mut shared_operations = Vec::new();
            for (key, entry) in &removed {
                remove_variant_key(&mut index.variants, &entry.base_key, key);
                shared_operations.push(SharedIndexOperation::RemoveEntry { key: key.clone() });
                shared_operations
                    .push(SharedIndexOperation::RemoveAdmissionCount { key: key.clone() });
            }
            apply_zone_shared_index_operations_locked(zone.as_ref(), &shared_operations);
            (removed, has_more_due)
        };
        if !removed.is_empty() {
            changed = true;
            total_removed += removed.len();
            for (key, _) in &removed {
                zone.remove_hot_entry(key);
            }
            for hash in removed
                .into_iter()
                .map(|(_, entry)| entry.hash)
                .collect::<std::collections::BTreeSet<_>>()
            {
                remove_cache_files_if_unreferenced(zone.as_ref(), &hash).await;
            }
        }
        if !has_more_due {
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
        let _sync_guard = zone.shared_index_sync_lock.lock().await;
        let mut index = lock_index(&zone.index);
        let matching_keys = index
            .entries
            .keys()
            .filter_map(|key| purge_selector_matches(&selector, key).then_some(key.clone()))
            .collect::<Vec<_>>();
        let mut removed = Vec::with_capacity(matching_keys.len());
        let mut shared_operations = Vec::with_capacity(matching_keys.len() * 2);
        for key in matching_keys {
            if let Some(entry) = index.remove_entry(&key) {
                index.current_size_bytes =
                    index.current_size_bytes.saturating_sub(entry.body_size_bytes);
                index.admission_counts.remove(&key);
                remove_variant_key(&mut index.variants, &entry.base_key, &key);
                shared_operations.push(SharedIndexOperation::RemoveEntry { key: key.clone() });
                shared_operations
                    .push(SharedIndexOperation::RemoveAdmissionCount { key: key.clone() });
                removed.push((key, entry));
            }
        }
        apply_zone_shared_index_operations_locked(zone.as_ref(), &shared_operations);
        removed
    };

    let removed_bytes = removed.iter().map(|(_, entry)| entry.body_size_bytes).sum::<usize>();
    let removed_entries = removed.len();
    if !removed.is_empty() {
        zone.record_purge(removed_entries);
        for (key, _) in &removed {
            zone.remove_hot_entry(key);
        }
        for hash in removed
            .into_iter()
            .map(|(_, entry)| entry.hash)
            .collect::<std::collections::BTreeSet<_>>()
        {
            remove_cache_files_if_unreferenced(zone.as_ref(), &hash).await;
        }
        zone.notify_changed();
    }
    CachePurgeResult { zone_name: zone.config.name.clone(), scope, removed_entries, removed_bytes }
}

pub(in crate::cache) async fn invalidate_zone_entries(
    zone: Arc<CacheZoneRuntime>,
    selector: CacheInvalidationSelector,
) -> CacheInvalidationResult {
    let scope = invalidation_scope(&selector);
    let rule =
        CacheInvalidationRule { selector, created_at_unix_ms: unix_time_ms(SystemTime::now()) };
    let (affected_entries, affected_bytes, affected_keys, active_rules) = {
        let _sync_guard = zone.shared_index_sync_lock.lock().await;
        let mut index = lock_index(&zone.index);
        let mut affected_entries = 0usize;
        let mut affected_bytes = 0usize;
        let mut affected_keys = Vec::new();
        for (key, entry) in &index.entries {
            if invalidation_rule_matches_entry(&rule, key, entry) {
                affected_entries = affected_entries.saturating_add(1);
                affected_bytes = affected_bytes.saturating_add(entry.body_size_bytes);
                affected_keys.push(key.clone());
            }
        }
        index.invalidations.push(rule.clone());
        let active_rules = index.invalidations.len();
        apply_zone_shared_index_operations_locked(
            zone.as_ref(),
            &[SharedIndexOperation::AddInvalidation { rule }],
        );
        (affected_entries, affected_bytes, affected_keys, active_rules)
    };

    for key in affected_keys {
        zone.remove_hot_entry(&key);
    }
    zone.record_invalidation(affected_entries);
    zone.notify_changed();
    CacheInvalidationResult {
        zone_name: zone.config.name.clone(),
        scope,
        affected_entries,
        affected_bytes,
        active_rules,
    }
}

pub(in crate::cache) async fn clear_zone_invalidations(
    zone: Arc<CacheZoneRuntime>,
) -> CacheInvalidationResult {
    let (affected_entries, affected_bytes, affected_keys) = {
        let _sync_guard = zone.shared_index_sync_lock.lock().await;
        let mut index = lock_index(&zone.index);
        let mut affected_entries = 0usize;
        let mut affected_bytes = 0usize;
        let mut affected_keys = Vec::new();
        for (key, entry) in &index.entries {
            if entry_is_logically_invalid(&index, key, entry) {
                affected_entries = affected_entries.saturating_add(1);
                affected_bytes = affected_bytes.saturating_add(entry.body_size_bytes);
                affected_keys.push(key.clone());
            }
        }
        index.invalidations.clear();
        apply_zone_shared_index_operations_locked(
            zone.as_ref(),
            &[SharedIndexOperation::ClearInvalidations],
        );
        (affected_entries, affected_bytes, affected_keys)
    };

    for key in affected_keys {
        zone.remove_hot_entry(&key);
    }
    zone.notify_changed();
    CacheInvalidationResult {
        zone_name: zone.config.name.clone(),
        scope: "clear".to_string(),
        affected_entries,
        affected_bytes,
        active_rules: 0,
    }
}
