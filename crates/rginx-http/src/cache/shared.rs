use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::invalidation::invalidation_rule_matches_entry;
use super::load::load_index_from_disk;
use super::store::lock_index;
use super::{CacheIndex, CacheIndexEntry, CacheInvalidationRule, CacheZoneRuntime};

mod index_file;

pub(super) use index_file::SharedIndexStore;
pub(super) use index_file::shared_index_store;
use index_file::{
    apply_shared_index_operations, delete_legacy_shared_index_file,
    load_legacy_shared_index_from_disk, load_shared_index_changes_since,
    load_shared_index_from_disk, recreate_shared_index_on_disk, shared_index_path,
    shared_index_sync_state,
};
pub(in crate::cache) use index_file::{
    run_blocking, shared_fill_lock_path, shared_fill_state_path,
};

type SharedIndexBootstrap = (CacheIndex, Option<Arc<SharedIndexStore>>, u64, u64, u64);

#[derive(Clone)]
pub(super) enum SharedIndexOperation {
    UpsertEntry { key: String, entry: CacheIndexEntry },
    RemoveEntry { key: String },
    SetAdmissionCount { key: String, uses: u64 },
    RemoveAdmissionCount { key: String },
    AddInvalidation { rule: CacheInvalidationRule },
    ClearInvalidations,
}

pub(super) fn bootstrap_shared_index(
    zone: &rginx_core::CacheZone,
) -> io::Result<SharedIndexBootstrap> {
    if !zone.shared_index {
        return Ok((load_index_from_disk(zone)?, None, 0, 0, 0));
    }

    let store = Arc::new(shared_index_store(zone));
    match run_blocking(|| load_shared_index_from_disk(store.as_ref())) {
        Ok(loaded) if shared_index_loaded(&loaded.index, loaded.generation) => {
            let _ = delete_legacy_shared_index_file(zone);
            Ok((
                loaded.index,
                Some(store),
                loaded.generation,
                loaded.store_epoch,
                loaded.last_change_seq,
            ))
        }
        Ok(_) => bootstrap_shared_index_from_cache_files(zone, store),
        Err(error) => {
            tracing::warn!(
                zone = %zone.name,
                path = %shared_index_path(zone).display(),
                %error,
                "failed to load shared cache index metadata; rebuilding from cache files"
            );
            bootstrap_shared_index_from_cache_files(zone, store)
        }
    }
}

pub(super) async fn sync_zone_shared_index_if_needed(zone: &Arc<CacheZoneRuntime>) {
    let Some(store) = zone.shared_index_store.as_ref() else {
        return;
    };

    let local_generation = zone.shared_index_generation.load(Ordering::Relaxed);
    let local_store_epoch = zone.shared_index_store_epoch.load(Ordering::Relaxed);
    let shared_state = match run_blocking(|| shared_index_sync_state(store.as_ref())) {
        Ok(state) => state,
        Err(error) => {
            tracing::warn!(
                zone = %zone.config.name,
                path = %store.path().display(),
                %error,
                "failed to read shared cache index state; keeping local index"
            );
            return;
        }
    };
    if shared_state.store_epoch == local_store_epoch && shared_state.generation <= local_generation
    {
        return;
    }

    let _sync_guard = zone.shared_index_sync_lock.lock().await;
    let local_generation = zone.shared_index_generation.load(Ordering::Relaxed);
    let local_store_epoch = zone.shared_index_store_epoch.load(Ordering::Relaxed);
    let local_change_seq = zone.shared_index_change_seq.load(Ordering::Relaxed);
    let shared_state = match run_blocking(|| shared_index_sync_state(store.as_ref())) {
        Ok(state) => state,
        Err(error) => {
            tracing::warn!(
                zone = %zone.config.name,
                path = %store.path().display(),
                %error,
                "failed to re-read shared cache index state; keeping local index"
            );
            return;
        }
    };
    if shared_state.store_epoch == local_store_epoch && shared_state.generation <= local_generation
    {
        return;
    }

    if shared_state.store_epoch != local_store_epoch {
        return reload_zone_shared_index(zone, store.as_ref());
    }

    let delta =
        match run_blocking(|| load_shared_index_changes_since(store.as_ref(), local_change_seq)) {
            Ok(delta) => delta,
            Err(error) => {
                tracing::warn!(
                    zone = %zone.config.name,
                    path = %store.path().display(),
                    %error,
                    "failed to read shared cache index delta; falling back to full reload"
                );
                return reload_zone_shared_index(zone, store.as_ref());
            }
        };

    if delta.store_epoch == local_store_epoch
        && delta.generation > local_generation
        && delta.last_change_seq >= local_change_seq
        && !delta.operations.is_empty()
    {
        let mut index = lock_index(&zone.index);
        apply_shared_index_delta(zone.as_ref(), &mut index, &delta.operations);
        drop(index);
        zone.shared_index_generation.store(delta.generation, Ordering::Relaxed);
        zone.shared_index_store_epoch.store(delta.store_epoch, Ordering::Relaxed);
        zone.shared_index_change_seq.store(delta.last_change_seq, Ordering::Relaxed);
        return;
    }

    reload_zone_shared_index(zone, store.as_ref());
}

pub(super) fn apply_zone_shared_index_operations_locked(
    zone: &CacheZoneRuntime,
    operations: &[SharedIndexOperation],
) {
    if operations.is_empty() {
        return;
    }
    let Some(store) = zone.shared_index_store.as_ref() else {
        return;
    };
    match run_blocking(|| apply_shared_index_operations(store.as_ref(), operations)) {
        Ok(applied) => {
            zone.shared_index_generation.store(applied.generation, Ordering::Relaxed);
            zone.shared_index_store_epoch.store(applied.store_epoch, Ordering::Relaxed);
            zone.shared_index_change_seq.store(applied.last_change_seq, Ordering::Relaxed);
        }
        Err(error) => {
            tracing::warn!(
                zone = %zone.config.name,
                path = %store.path().display(),
                %error,
                "failed to apply shared cache index metadata update"
            );
        }
    }
}

fn bootstrap_shared_index_from_cache_files(
    zone: &rginx_core::CacheZone,
    store: Arc<SharedIndexStore>,
) -> io::Result<SharedIndexBootstrap> {
    match load_legacy_shared_index_from_disk(zone) {
        Ok(Some(legacy)) => {
            let generation = legacy.generation.max(1);
            let rebuilt = rebuild_shared_index_store(store.as_ref(), &legacy.index, generation)?;
            let _ = delete_legacy_shared_index_file(zone);
            return Ok((
                legacy.index,
                Some(store),
                rebuilt.generation,
                rebuilt.store_epoch,
                rebuilt.last_change_seq,
            ));
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(
                zone = %zone.name,
                path = %shared_index_path(zone).display(),
                %error,
                "failed to import legacy shared cache index; rebuilding from cache files"
            );
        }
    }

    let index = load_index_from_disk(zone)?;
    let generation = 1;
    let rebuilt = rebuild_shared_index_store(store.as_ref(), &index, generation)?;
    Ok((index, Some(store), rebuilt.generation, rebuilt.store_epoch, rebuilt.last_change_seq))
}

fn rebuild_shared_index_store(
    store: &SharedIndexStore,
    index: &CacheIndex,
    generation: u64,
) -> io::Result<index_file::AppliedSharedIndexOperations> {
    recreate_shared_index_on_disk(store, index, generation)
}

fn shared_index_loaded(index: &CacheIndex, generation: u64) -> bool {
    generation > 0
        || !index.entries.is_empty()
        || !index.admission_counts.is_empty()
        || !index.invalidations.is_empty()
}

fn reload_zone_shared_index(zone: &Arc<CacheZoneRuntime>, store: &SharedIndexStore) {
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

fn apply_shared_index_delta(
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
