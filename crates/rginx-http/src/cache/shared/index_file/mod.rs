use std::fs;
use std::io;
use std::path::PathBuf;

use super::super::CacheIndex;
use super::super::entry::cache_key_hash;
use super::SharedIndexOperation;

mod codec;
#[cfg(target_os = "linux")]
mod memory_backend;
#[cfg(not(target_os = "linux"))]
mod memory_backend {
    use std::io;
    use std::path::{Path, PathBuf};

    use super::super::CacheIndex;
    use super::SharedIndexOperation;
    use super::{
        AppliedSharedIndexOperations, LoadedSharedIndex, LoadedSharedIndexChanges,
        SharedIndexBackend, SharedIndexMetrics, SharedIndexSyncState,
    };

    pub(super) struct MemorySharedIndexStore {
        path: PathBuf,
    }

    impl MemorySharedIndexStore {
        pub(super) fn new(zone: &rginx_core::CacheZone) -> Self {
            Self { path: zone.path.join(".rginx-index.shm") }
        }
    }

    impl SharedIndexBackend for MemorySharedIndexStore {
        fn path(&self) -> &Path {
            &self.path
        }

        fn load(&self) -> io::Result<LoadedSharedIndex> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "shared memory index is only supported on linux",
            ))
        }

        fn sync_state(&self) -> io::Result<SharedIndexSyncState> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "shared memory index is only supported on linux",
            ))
        }

        fn load_changes_since(&self, _after_seq: u64) -> io::Result<LoadedSharedIndexChanges> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "shared memory index is only supported on linux",
            ))
        }

        fn recreate(
            &self,
            _index: &CacheIndex,
            _generation: u64,
        ) -> io::Result<AppliedSharedIndexOperations> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "shared memory index is only supported on linux",
            ))
        }

        fn apply_operations(
            &self,
            _operations: &[SharedIndexOperation],
        ) -> io::Result<AppliedSharedIndexOperations> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "shared memory index is only supported on linux",
            ))
        }

        fn supports_shared_fill_locks(&self) -> bool {
            false
        }

        fn metrics(&self) -> io::Result<SharedIndexMetrics> {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "shared memory index is only supported on linux",
            ))
        }
    }
}

const LEGACY_SHARED_INDEX_FILENAME: &str = ".rginx-index.json";
const SHARED_FILL_LOCK_PREFIX: &str = ".rginx-fill-";
const SHARED_FILL_LOCK_SUFFIX: &str = ".lock";
const SHARED_FILL_STATE_SUFFIX: &str = ".state.json";
const SHARED_INDEX_SCHEMA_VERSION: u8 = 2;

pub(super) struct LoadedSharedIndex {
    pub(super) index: CacheIndex,
    pub(super) generation: u64,
    pub(super) store_epoch: u64,
    pub(super) last_change_seq: u64,
}

pub(super) struct LoadedSharedIndexChanges {
    pub(super) generation: u64,
    pub(super) store_epoch: u64,
    pub(super) last_change_seq: u64,
    pub(super) operations: Vec<SharedIndexOperation>,
}

pub(super) struct AppliedSharedIndexOperations {
    pub(super) generation: u64,
    pub(super) store_epoch: u64,
    pub(super) last_change_seq: u64,
}

pub(super) struct SharedIndexSyncState {
    pub(super) generation: u64,
    pub(super) store_epoch: u64,
}

#[derive(Debug, Clone, Default)]
pub(in crate::cache) struct SharedIndexMetrics {
    pub(in crate::cache) shm_capacity_bytes: u64,
    pub(in crate::cache) shm_used_bytes: u64,
    pub(in crate::cache) entry_count: u64,
    pub(in crate::cache) current_size_bytes: u64,
    pub(in crate::cache) operation_ring_capacity: u64,
    pub(in crate::cache) operation_ring_used: u64,
    pub(in crate::cache) lock_contention_total: u64,
    pub(in crate::cache) full_reload_total: u64,
    pub(in crate::cache) rebuild_total: u64,
    pub(in crate::cache) stale_fill_lock_cleanup_total: u64,
    pub(in crate::cache) capacity_rejection_total: u64,
}

pub(in crate::cache) struct SharedFillLockSnapshot {
    pub(in crate::cache) nonce: String,
    pub(in crate::cache) state_json: Vec<u8>,
}

pub(in crate::cache) enum SharedFillLockAcquire {
    Acquired,
    Busy,
}

pub(in crate::cache) enum SharedFillLockStatus {
    Missing,
    Fresh,
    Stale,
}

trait SharedIndexBackend: Send + Sync {
    fn path(&self) -> &std::path::Path;
    fn load(&self) -> io::Result<LoadedSharedIndex>;
    fn sync_state(&self) -> io::Result<SharedIndexSyncState>;
    fn load_changes_since(&self, after_seq: u64) -> io::Result<LoadedSharedIndexChanges>;
    fn recreate(
        &self,
        index: &CacheIndex,
        generation: u64,
    ) -> io::Result<AppliedSharedIndexOperations>;
    fn apply_operations(
        &self,
        operations: &[SharedIndexOperation],
    ) -> io::Result<AppliedSharedIndexOperations>;

    fn supports_shared_fill_locks(&self) -> bool {
        false
    }

    fn metrics(&self) -> io::Result<SharedIndexMetrics> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "shared index metrics are not supported by this backend",
        ))
    }

    fn try_acquire_fill_lock(
        &self,
        _key: &str,
        _now_unix_ms: u64,
        _lock_age_ms: u64,
        _nonce: &str,
        _state_json: &[u8],
    ) -> io::Result<SharedFillLockAcquire> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "shared fill locks are not supported by this backend",
        ))
    }

    fn load_fill_lock(&self, _key: &str) -> io::Result<Option<SharedFillLockSnapshot>> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "shared fill locks are not supported by this backend",
        ))
    }

    fn update_fill_lock(
        &self,
        _key: &str,
        _nonce: &str,
        _updated_at_unix_ms: u64,
        _state_json: &[u8],
    ) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "shared fill locks are not supported by this backend",
        ))
    }

    fn release_fill_lock(&self, _key: &str, _nonce: &str) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "shared fill locks are not supported by this backend",
        ))
    }

    fn fill_lock_status(
        &self,
        _key: &str,
        _now_unix_ms: u64,
        _lock_age_ms: u64,
    ) -> io::Result<SharedFillLockStatus> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "shared fill locks are not supported by this backend",
        ))
    }

    fn clear_stale_fill_lock(
        &self,
        _key: &str,
        _now_unix_ms: u64,
        _lock_age_ms: u64,
    ) -> io::Result<bool> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "shared fill locks are not supported by this backend",
        ))
    }
}

pub(in crate::cache) struct SharedIndexStore {
    backend: Box<dyn SharedIndexBackend>,
}

impl SharedIndexStore {
    pub(in crate::cache) fn for_zone(zone: &rginx_core::CacheZone) -> Self {
        Self { backend: Box::new(memory_backend::MemorySharedIndexStore::new(zone)) }
    }

    pub(in crate::cache) fn path(&self) -> &std::path::Path {
        self.backend.path()
    }

    pub(super) fn load(&self) -> io::Result<LoadedSharedIndex> {
        self.backend.load()
    }

    pub(super) fn sync_state(&self) -> io::Result<SharedIndexSyncState> {
        self.backend.sync_state()
    }

    pub(super) fn load_changes_since(
        &self,
        after_seq: u64,
    ) -> io::Result<LoadedSharedIndexChanges> {
        self.backend.load_changes_since(after_seq)
    }

    pub(super) fn recreate(
        &self,
        index: &CacheIndex,
        generation: u64,
    ) -> io::Result<AppliedSharedIndexOperations> {
        self.backend.recreate(index, generation)
    }

    pub(super) fn apply_operations(
        &self,
        operations: &[SharedIndexOperation],
    ) -> io::Result<AppliedSharedIndexOperations> {
        self.backend.apply_operations(operations)
    }

    pub(in crate::cache) fn supports_shared_fill_locks(&self) -> bool {
        self.backend.supports_shared_fill_locks()
    }

    pub(in crate::cache) fn metrics(&self) -> io::Result<SharedIndexMetrics> {
        self.backend.metrics()
    }

    pub(in crate::cache) fn try_acquire_fill_lock(
        &self,
        key: &str,
        now_unix_ms: u64,
        lock_age_ms: u64,
        nonce: &str,
        state_json: &[u8],
    ) -> io::Result<SharedFillLockAcquire> {
        self.backend.try_acquire_fill_lock(key, now_unix_ms, lock_age_ms, nonce, state_json)
    }

    pub(in crate::cache) fn load_fill_lock(
        &self,
        key: &str,
    ) -> io::Result<Option<SharedFillLockSnapshot>> {
        self.backend.load_fill_lock(key)
    }

    pub(in crate::cache) fn update_fill_lock(
        &self,
        key: &str,
        nonce: &str,
        updated_at_unix_ms: u64,
        state_json: &[u8],
    ) -> io::Result<()> {
        self.backend.update_fill_lock(key, nonce, updated_at_unix_ms, state_json)
    }

    pub(in crate::cache) fn release_fill_lock(&self, key: &str, nonce: &str) -> io::Result<()> {
        self.backend.release_fill_lock(key, nonce)
    }

    pub(in crate::cache) fn fill_lock_status(
        &self,
        key: &str,
        now_unix_ms: u64,
        lock_age_ms: u64,
    ) -> io::Result<SharedFillLockStatus> {
        self.backend.fill_lock_status(key, now_unix_ms, lock_age_ms)
    }

    pub(in crate::cache) fn clear_stale_fill_lock(
        &self,
        key: &str,
        now_unix_ms: u64,
        lock_age_ms: u64,
    ) -> io::Result<bool> {
        self.backend.clear_stale_fill_lock(key, now_unix_ms, lock_age_ms)
    }
}

pub(in crate::cache) fn shared_fill_lock_path(zone: &rginx_core::CacheZone, key: &str) -> PathBuf {
    zone.path
        .join(format!("{SHARED_FILL_LOCK_PREFIX}{}{SHARED_FILL_LOCK_SUFFIX}", cache_key_hash(key)))
}

pub(in crate::cache) fn shared_fill_state_path(zone: &rginx_core::CacheZone, key: &str) -> PathBuf {
    zone.path
        .join(format!("{SHARED_FILL_LOCK_PREFIX}{}{SHARED_FILL_STATE_SUFFIX}", cache_key_hash(key)))
}

pub(super) fn legacy_shared_index_path(zone: &rginx_core::CacheZone) -> std::path::PathBuf {
    zone.path.join(LEGACY_SHARED_INDEX_FILENAME)
}

pub(super) fn load_legacy_shared_index_from_disk(
    zone: &rginx_core::CacheZone,
) -> io::Result<Option<LoadedSharedIndex>> {
    let path = legacy_shared_index_path(zone);
    if !path.exists() {
        return Ok(None);
    }

    let bytes = fs::read(&path)?;
    codec::load_legacy_shared_index_bytes(&bytes, &path).map(Some)
}

pub(super) fn delete_legacy_shared_index_file(zone: &rginx_core::CacheZone) -> io::Result<()> {
    let path = legacy_shared_index_path(zone);
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(in crate::cache) fn shared_index_store(zone: &rginx_core::CacheZone) -> SharedIndexStore {
    SharedIndexStore::for_zone(zone)
}

#[cfg(test)]
pub(in crate::cache) fn unlink_memory_shared_index_for_test(
    zone: &rginx_core::CacheZone,
) -> io::Result<()> {
    memory_backend::unlink_for_zone(zone)
}

#[cfg(all(test, target_os = "linux"))]
pub(in crate::cache) fn corrupt_memory_shared_index_for_test(
    zone: &rginx_core::CacheZone,
) -> io::Result<()> {
    memory_backend::corrupt_header_for_zone(zone)
}

#[cfg(all(test, target_os = "linux"))]
pub(in crate::cache) fn corrupt_memory_shared_index_document_for_test(
    zone: &rginx_core::CacheZone,
) -> io::Result<()> {
    memory_backend::corrupt_document_for_zone(zone)
}

pub(super) fn load_shared_index_from_disk(
    store: &SharedIndexStore,
) -> io::Result<LoadedSharedIndex> {
    store.load()
}

pub(super) fn recreate_shared_index_on_disk(
    store: &SharedIndexStore,
    index: &CacheIndex,
    generation: u64,
) -> io::Result<AppliedSharedIndexOperations> {
    store.recreate(index, generation)
}

pub(super) fn shared_index_sync_state(
    store: &SharedIndexStore,
) -> io::Result<SharedIndexSyncState> {
    store.sync_state()
}

pub(super) fn load_shared_index_changes_since(
    store: &SharedIndexStore,
    after_seq: u64,
) -> io::Result<LoadedSharedIndexChanges> {
    store.load_changes_since(after_seq)
}

pub(super) fn apply_shared_index_operations(
    store: &SharedIndexStore,
    operations: &[SharedIndexOperation],
) -> io::Result<AppliedSharedIndexOperations> {
    store.apply_operations(operations)
}

pub(in crate::cache) fn run_blocking<T>(
    operation: impl FnOnce() -> io::Result<T>,
) -> io::Result<T> {
    if let Ok(handle) = tokio::runtime::Handle::try_current()
        && handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
    {
        return tokio::task::block_in_place(operation);
    }
    operation()
}

fn invalid_data_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
