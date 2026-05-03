use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use super::super::super::CacheIndex;
use super::super::SharedIndexOperation;
use super::codec::{
    deserialize_entry_record, deserialize_invalidation_rule, serialize_entry_record,
    serialize_invalidation_rule,
};
use super::schema::{
    initialize_schema, read_meta_u64, read_meta_u64_tx, reset_schema, write_meta_u64,
};
use super::{
    AppliedSharedIndexOperations, LoadedSharedIndex, LoadedSharedIndexChanges, META_GENERATION_KEY,
    META_STORE_EPOCH_KEY, SharedIndexSyncState, io_error,
};

mod apply;
mod load;

use apply::{append_changes_tx, apply_operations_tx, replace_all};
use load::{
    load_admission_counts_into_index, load_changes_since_tx, load_entries_into_index,
    load_invalidations_into_index, read_max_change_seq_tx,
};

pub(super) const CHANGE_OP_UPSERT_ENTRY: u64 = 1;
pub(super) const CHANGE_OP_REMOVE_ENTRY: u64 = 2;
pub(super) const CHANGE_OP_SET_ADMISSION_COUNT: u64 = 3;
pub(super) const CHANGE_OP_REMOVE_ADMISSION_COUNT: u64 = 4;
pub(super) const CHANGE_OP_ADD_INVALIDATION: u64 = 5;
pub(super) const CHANGE_OP_CLEAR_INVALIDATIONS: u64 = 6;

pub(in crate::cache) struct SharedIndexStore {
    path: PathBuf,
}

impl SharedIndexStore {
    pub(in crate::cache) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(in crate::cache) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn load(&self) -> io::Result<LoadedSharedIndex> {
        let mut connection = self.open_connection()?;
        let transaction = connection.transaction().map_err(io_error)?;
        let generation = read_meta_u64_tx(&transaction, META_GENERATION_KEY)?.unwrap_or_default();
        let store_epoch = read_meta_u64_tx(&transaction, META_STORE_EPOCH_KEY)?.unwrap_or(1);
        let last_change_seq = read_max_change_seq_tx(&transaction)?;
        let mut index = CacheIndex::default();
        load_entries_into_index(&transaction, &mut index)?;
        load_admission_counts_into_index(&transaction, &mut index)?;
        load_invalidations_into_index(&transaction, &mut index)?;
        Ok(LoadedSharedIndex { index, generation, store_epoch, last_change_seq })
    }

    pub(super) fn sync_state(&self) -> io::Result<SharedIndexSyncState> {
        let connection = self.open_connection()?;
        Ok(SharedIndexSyncState {
            generation: read_meta_u64(&connection, META_GENERATION_KEY)?.unwrap_or_default(),
            store_epoch: read_meta_u64(&connection, META_STORE_EPOCH_KEY)?.unwrap_or(1),
        })
    }

    pub(super) fn load_changes_since(
        &self,
        after_seq: u64,
    ) -> io::Result<LoadedSharedIndexChanges> {
        let mut connection = self.open_connection()?;
        let transaction = connection.transaction().map_err(io_error)?;
        let generation = read_meta_u64_tx(&transaction, META_GENERATION_KEY)?.unwrap_or_default();
        let store_epoch = read_meta_u64_tx(&transaction, META_STORE_EPOCH_KEY)?.unwrap_or(1);
        let last_change_seq = read_max_change_seq_tx(&transaction)?;
        let operations = load_changes_since_tx(&transaction, after_seq)?;
        Ok(LoadedSharedIndexChanges { generation, store_epoch, last_change_seq, operations })
    }

    pub(super) fn recreate(
        &self,
        index: &CacheIndex,
        generation: u64,
    ) -> io::Result<AppliedSharedIndexOperations> {
        let mut connection = match self.open_raw_connection() {
            Ok(connection) => connection,
            Err(first_error) => {
                let _ = fs::remove_file(&self.path);
                self.open_raw_connection().map_err(|_| first_error)?
            }
        };

        if initialize_schema(&connection).is_err() {
            reset_schema(&mut connection)?;
        }

        replace_all(&mut connection, index, generation)
    }

    pub(super) fn apply_operations(
        &self,
        operations: &[SharedIndexOperation],
    ) -> io::Result<AppliedSharedIndexOperations> {
        if operations.is_empty() {
            let loaded = self.load()?;
            return Ok(AppliedSharedIndexOperations {
                generation: loaded.generation,
                store_epoch: loaded.store_epoch,
                last_change_seq: loaded.last_change_seq,
            });
        }

        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io_error)?;
        let current_generation =
            read_meta_u64_tx(&transaction, META_GENERATION_KEY)?.unwrap_or_default();
        let store_epoch = read_meta_u64_tx(&transaction, META_STORE_EPOCH_KEY)?.unwrap_or(1);
        let next_generation = current_generation.saturating_add(1);
        apply_operations_tx(&transaction, operations)?;
        append_changes_tx(&transaction, next_generation, operations)?;
        write_meta_u64(&transaction, META_GENERATION_KEY, next_generation)?;
        let last_change_seq = read_max_change_seq_tx(&transaction)?;
        transaction.commit().map_err(io_error)?;
        Ok(AppliedSharedIndexOperations {
            generation: next_generation,
            store_epoch,
            last_change_seq,
        })
    }

    fn open_connection(&self) -> io::Result<Connection> {
        let connection = self.open_raw_connection()?;
        initialize_schema(&connection)?;
        Ok(connection)
    }

    fn open_raw_connection(&self) -> io::Result<Connection> {
        let connection = Connection::open(&self.path).map_err(io_error)?;
        connection.busy_timeout(Duration::from_secs(5)).map_err(io_error)?;
        Ok(connection)
    }
}
