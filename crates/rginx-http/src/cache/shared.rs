use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::load::load_index_from_disk;
use super::store::lock_index;
use super::{CacheIndex, CacheIndexEntry, CacheZoneRuntime};

mod index_file;

pub(super) use index_file::SharedIndexStore;
pub(super) use index_file::shared_index_store;
use index_file::{
    apply_shared_index_operations, delete_legacy_shared_index_file,
    load_legacy_shared_index_from_disk, load_shared_index_from_disk, recreate_shared_index_on_disk,
    shared_index_generation, shared_index_path,
};
pub(in crate::cache) use index_file::{
    run_blocking, shared_fill_lock_path, shared_fill_state_path,
};

#[derive(Clone)]
pub(super) enum SharedIndexOperation {
    UpsertEntry { key: String, entry: CacheIndexEntry },
    RemoveEntry { key: String },
    SetAdmissionCount { key: String, uses: u64 },
    RemoveAdmissionCount { key: String },
}

pub(super) fn bootstrap_shared_index(
    zone: &rginx_core::CacheZone,
) -> io::Result<(CacheIndex, Option<Arc<SharedIndexStore>>, u64)> {
    if !zone.shared_index {
        return Ok((load_index_from_disk(zone)?, None, 0));
    }

    let store = Arc::new(shared_index_store(zone));
    match run_blocking(|| load_shared_index_from_disk(store.as_ref())) {
        Ok(loaded) if shared_index_loaded(&loaded.index, loaded.generation) => {
            let _ = delete_legacy_shared_index_file(zone);
            Ok((loaded.index, Some(store), loaded.generation))
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
    let shared_generation = match run_blocking(|| shared_index_generation(store.as_ref())) {
        Ok(generation) => generation,
        Err(error) => {
            tracing::warn!(
                zone = %zone.config.name,
                path = %store.path().display(),
                %error,
                "failed to read shared cache index generation; keeping local index"
            );
            return;
        }
    };
    if shared_generation <= local_generation {
        return;
    }

    let _sync_guard = zone.shared_index_sync_lock.lock().await;
    let local_generation = zone.shared_index_generation.load(Ordering::Relaxed);
    let shared_generation = match run_blocking(|| shared_index_generation(store.as_ref())) {
        Ok(generation) => generation,
        Err(error) => {
            tracing::warn!(
                zone = %zone.config.name,
                path = %store.path().display(),
                %error,
                "failed to re-read shared cache index generation; keeping local index"
            );
            return;
        }
    };
    if shared_generation <= local_generation {
        return;
    }

    let loaded = match run_blocking(|| load_shared_index_from_disk(store.as_ref())) {
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
    zone.shared_index_generation.store(loaded.generation, Ordering::Relaxed);
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
        Ok(generation) => {
            zone.shared_index_generation.store(generation, Ordering::Relaxed);
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
) -> io::Result<(CacheIndex, Option<Arc<SharedIndexStore>>, u64)> {
    match load_legacy_shared_index_from_disk(zone) {
        Ok(Some(legacy)) => {
            let generation = legacy.generation.max(1);
            rebuild_shared_index_store(store.as_ref(), &legacy.index, generation)?;
            let _ = delete_legacy_shared_index_file(zone);
            return Ok((legacy.index, Some(store), generation));
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
    rebuild_shared_index_store(store.as_ref(), &index, generation)?;
    Ok((index, Some(store), generation))
}

fn rebuild_shared_index_store(
    store: &SharedIndexStore,
    index: &CacheIndex,
    generation: u64,
) -> io::Result<()> {
    recreate_shared_index_on_disk(store, index, generation)?;
    Ok(())
}

fn shared_index_loaded(index: &CacheIndex, generation: u64) -> bool {
    generation > 0 || !index.entries.is_empty() || !index.admission_counts.is_empty()
}
