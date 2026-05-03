use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::invalidation::invalidation_rule_matches_entry;
use super::load::load_index_from_disk;
use super::store::lock_index;
use super::{CacheIndex, CacheIndexEntry, CacheInvalidationRule, CacheZoneRuntime};

mod bootstrap;
mod delta;
#[cfg(target_os = "linux")]
pub(super) mod memory;
#[cfg(not(target_os = "linux"))]
pub(super) mod memory {}
mod index_file;

pub(in crate::cache) use bootstrap::bootstrap_shared_index;
use delta::{apply_shared_index_delta, reload_zone_shared_index};
pub(super) use index_file::SharedIndexStore;
#[cfg(all(test, target_os = "linux"))]
pub(in crate::cache) use index_file::corrupt_memory_shared_index_document_for_test;
#[cfg(all(test, target_os = "linux"))]
pub(in crate::cache) use index_file::corrupt_memory_shared_index_for_test;
pub(super) use index_file::shared_index_store;
#[cfg(all(test, target_os = "linux"))]
pub(in crate::cache) use index_file::unlink_memory_shared_index_for_test;
pub(in crate::cache) use index_file::{SharedFillLockAcquire, SharedFillLockStatus};
use index_file::{
    apply_shared_index_operations, delete_legacy_shared_index_file, legacy_shared_index_path,
    load_legacy_shared_index_from_disk, load_shared_index_changes_since,
    load_shared_index_from_disk, recreate_shared_index_on_disk, shared_index_sync_state,
};
pub(in crate::cache) use index_file::{
    run_blocking, shared_fill_lock_path, shared_fill_state_path,
};

type SharedIndexBootstrap = (CacheIndex, Option<Arc<SharedIndexStore>>, u64, u64, u64);

#[derive(Clone)]
pub(super) enum SharedIndexOperation {
    UpsertEntry { key: String, entry: CacheIndexEntry },
    RemoveEntry { key: String },
    TouchEntry { key: String, last_access_unix_ms: u64 },
    SetAdmissionCount { key: String, uses: u64 },
    RemoveAdmissionCount { key: String },
    AddInvalidation { rule: CacheInvalidationRule },
    ClearInvalidations,
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

pub(super) async fn record_zone_shared_entry_access(
    zone: &Arc<CacheZoneRuntime>,
    key: &str,
    last_access_unix_ms: u64,
) {
    let _sync_guard = zone.shared_index_sync_lock.lock().await;
    let should_publish = {
        let mut index = lock_index(&zone.index);
        let Some(entry) = index.entries.get_mut(key) else {
            return;
        };
        if entry.last_access_unix_ms >= last_access_unix_ms {
            false
        } else {
            entry.last_access_unix_ms = last_access_unix_ms;
            index.reschedule_entry_access(key, last_access_unix_ms);
            true
        }
    };
    if should_publish {
        apply_zone_shared_index_operations_locked(
            zone.as_ref(),
            &[SharedIndexOperation::TouchEntry { key: key.to_string(), last_access_unix_ms }],
        );
    }
}
