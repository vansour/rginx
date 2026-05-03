use std::fs;
use std::io;
use std::path::PathBuf;

use super::super::CacheIndex;
use super::super::entry::cache_key_hash;
use super::SharedIndexOperation;

mod codec;
mod schema;
mod sqlite;

pub(in crate::cache) use sqlite::SharedIndexStore;

const SHARED_INDEX_SCHEMA_VERSION: u64 = 2;
const SHARED_INDEX_FILENAME: &str = ".rginx-index.sqlite3";
const LEGACY_SHARED_INDEX_FILENAME: &str = ".rginx-index.json";
const SHARED_FILL_LOCK_PREFIX: &str = ".rginx-fill-";
const SHARED_FILL_LOCK_SUFFIX: &str = ".lock";
const SHARED_FILL_STATE_SUFFIX: &str = ".state.json";
const META_SCHEMA_VERSION_KEY: &str = "schema_version";
const META_GENERATION_KEY: &str = "generation";
const META_STORE_EPOCH_KEY: &str = "store_epoch";

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

pub(in crate::cache) fn shared_fill_lock_path(zone: &rginx_core::CacheZone, key: &str) -> PathBuf {
    zone.path
        .join(format!("{SHARED_FILL_LOCK_PREFIX}{}{SHARED_FILL_LOCK_SUFFIX}", cache_key_hash(key)))
}

pub(in crate::cache) fn shared_fill_state_path(zone: &rginx_core::CacheZone, key: &str) -> PathBuf {
    zone.path
        .join(format!("{SHARED_FILL_LOCK_PREFIX}{}{SHARED_FILL_STATE_SUFFIX}", cache_key_hash(key)))
}

pub(super) fn shared_index_path(zone: &rginx_core::CacheZone) -> PathBuf {
    zone.path.join(SHARED_INDEX_FILENAME)
}

pub(super) fn legacy_shared_index_path(zone: &rginx_core::CacheZone) -> PathBuf {
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
    SharedIndexStore::new(shared_index_path(zone))
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

fn io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}
