use std::io;

use super::load::load_index_from_disk;
use super::store::lock_index;
use super::{CacheIndex, CacheZoneRuntime};

mod index_file;

pub(in crate::cache) use index_file::shared_fill_lock_path;
use index_file::{
    load_shared_index_from_disk, persist_shared_index_to_disk, run_blocking,
    shared_index_modified_unix_ms, shared_index_path,
};

pub(super) fn bootstrap_shared_index(
    zone: &rginx_core::CacheZone,
) -> io::Result<(CacheIndex, u64, u64)> {
    if !zone.shared_index {
        return Ok((load_index_from_disk(zone)?, 0, 0));
    }

    match load_shared_index_from_disk(zone) {
        Ok(Some(loaded)) => Ok((loaded.index, loaded.generation, loaded.modified_unix_ms)),
        Ok(None) => {
            let index = load_index_from_disk(zone)?;
            let modified_unix_ms = persist_shared_index_to_disk(zone, &index, 1, 0)?;
            Ok((index, 1, modified_unix_ms))
        }
        Err(error) => {
            tracing::warn!(
                zone = %zone.name,
                path = %shared_index_path(zone).display(),
                %error,
                "failed to load shared cache index; rebuilding from cache files"
            );
            let index = load_index_from_disk(zone)?;
            let modified_unix_ms = persist_shared_index_to_disk(zone, &index, 1, 0)?;
            Ok((index, 1, modified_unix_ms))
        }
    }
}

pub(super) async fn sync_zone_shared_index_if_needed(zone: &std::sync::Arc<CacheZoneRuntime>) {
    if !zone.config.shared_index {
        return;
    }

    let Some(disk_modified_unix_ms) = read_shared_index_modified_unix_ms(zone.config.as_ref())
    else {
        return;
    };
    if disk_modified_unix_ms
        <= zone.shared_index_last_modified_unix_ms.load(std::sync::atomic::Ordering::Relaxed)
    {
        return;
    }

    let _sync_guard = zone.shared_index_sync_lock.lock().await;
    let Some(disk_modified_unix_ms) = read_shared_index_modified_unix_ms(zone.config.as_ref())
    else {
        return;
    };
    if disk_modified_unix_ms
        <= zone.shared_index_last_modified_unix_ms.load(std::sync::atomic::Ordering::Relaxed)
    {
        return;
    }

    let loaded = match run_blocking(|| load_shared_index_from_disk(zone.config.as_ref())) {
        Ok(Some(loaded)) => loaded,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(
                zone = %zone.config.name,
                path = %shared_index_path(zone.config.as_ref()).display(),
                %error,
                "failed to reload shared cache index; keeping local index"
            );
            return;
        }
    };

    let mut index = lock_index(&zone.index);
    *index = loaded.index;
    drop(index);
    zone.shared_index_generation.store(loaded.generation, std::sync::atomic::Ordering::Relaxed);
    zone.shared_index_last_modified_unix_ms
        .store(loaded.modified_unix_ms, std::sync::atomic::Ordering::Relaxed);
}

pub(super) async fn persist_zone_shared_index(zone: &std::sync::Arc<CacheZoneRuntime>) {
    if !zone.config.shared_index {
        return;
    }

    let _sync_guard = zone.shared_index_sync_lock.lock().await;
    let next_generation =
        zone.shared_index_generation.load(std::sync::atomic::Ordering::Relaxed) + 1;
    let minimum_modified_unix_ms = zone
        .shared_index_last_modified_unix_ms
        .load(std::sync::atomic::Ordering::Relaxed)
        .saturating_add(1);
    let snapshot = {
        let index = lock_index(&zone.index);
        index.clone()
    };
    match run_blocking(|| {
        persist_shared_index_to_disk(
            zone.config.as_ref(),
            &snapshot,
            next_generation,
            minimum_modified_unix_ms,
        )
    }) {
        Ok(modified_unix_ms) => {
            zone.shared_index_generation
                .store(next_generation, std::sync::atomic::Ordering::Relaxed);
            zone.shared_index_last_modified_unix_ms
                .store(modified_unix_ms, std::sync::atomic::Ordering::Relaxed);
        }
        Err(error) => {
            tracing::warn!(
                zone = %zone.config.name,
                path = %shared_index_path(zone.config.as_ref()).display(),
                %error,
                "failed to persist shared cache index"
            );
        }
    }
}

fn read_shared_index_modified_unix_ms(zone: &rginx_core::CacheZone) -> Option<u64> {
    run_blocking(|| Ok(shared_index_modified_unix_ms(zone))).ok().flatten()
}
