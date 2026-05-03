use super::*;

pub(in crate::cache) fn bootstrap_shared_index(
    zone: &rginx_core::CacheZone,
) -> io::Result<SharedIndexBootstrap> {
    if !zone.shared_index {
        return Ok((load_index_from_disk(zone)?, None, 0, 0, 0));
    }

    bootstrap_shared_index_with_store(zone, Arc::new(shared_index_store(zone)))
}

fn bootstrap_shared_index_with_store(
    zone: &rginx_core::CacheZone,
    store: Arc<SharedIndexStore>,
) -> io::Result<SharedIndexBootstrap> {
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
                path = %store.path().display(),
                %error,
                "failed to load shared cache index metadata; rebuilding from cache files"
            );
            bootstrap_shared_index_from_cache_files(zone, store)
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
                path = %legacy_shared_index_path(zone).display(),
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
