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

const CHANGE_OP_UPSERT_ENTRY: u64 = 1;
const CHANGE_OP_REMOVE_ENTRY: u64 = 2;
const CHANGE_OP_SET_ADMISSION_COUNT: u64 = 3;
const CHANGE_OP_REMOVE_ADMISSION_COUNT: u64 = 4;
const CHANGE_OP_ADD_INVALIDATION: u64 = 5;
const CHANGE_OP_CLEAR_INVALIDATIONS: u64 = 6;

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

fn replace_all(
    connection: &mut Connection,
    index: &CacheIndex,
    generation: u64,
) -> io::Result<AppliedSharedIndexOperations> {
    let transaction =
        connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(io_error)?;
    clear_index_tables(&transaction)?;
    insert_entries(&transaction, index)?;
    insert_admission_counts(&transaction, index)?;
    insert_invalidations(&transaction, index)?;
    write_meta_u64(&transaction, META_GENERATION_KEY, generation)?;
    let store_epoch = read_meta_u64_tx(&transaction, META_STORE_EPOCH_KEY)?.unwrap_or(1);
    let last_change_seq = read_max_change_seq_tx(&transaction)?;
    transaction.commit().map_err(io_error)?;
    Ok(AppliedSharedIndexOperations { generation, store_epoch, last_change_seq })
}

fn apply_operations_tx(
    transaction: &Transaction<'_>,
    operations: &[SharedIndexOperation],
) -> io::Result<()> {
    let mut upsert_entry = transaction
        .prepare(
            "INSERT INTO entries (key, entry_json) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET entry_json = excluded.entry_json",
        )
        .map_err(io_error)?;
    let mut remove_entry =
        transaction.prepare("DELETE FROM entries WHERE key = ?1").map_err(io_error)?;
    let mut set_admission = transaction
        .prepare(
            "INSERT INTO admission_counts (key, uses) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET uses = excluded.uses",
        )
        .map_err(io_error)?;
    let mut remove_admission =
        transaction.prepare("DELETE FROM admission_counts WHERE key = ?1").map_err(io_error)?;
    let mut insert_invalidation = transaction
        .prepare("INSERT INTO invalidations (rule_json) VALUES (?1)")
        .map_err(io_error)?;
    let mut clear_invalidations =
        transaction.prepare("DELETE FROM invalidations").map_err(io_error)?;

    for operation in operations {
        match operation {
            SharedIndexOperation::UpsertEntry { key, entry } => {
                let entry_json = serialize_entry_record(entry)?;
                upsert_entry.execute(params![key, entry_json]).map_err(io_error)?;
            }
            SharedIndexOperation::RemoveEntry { key } => {
                remove_entry.execute(params![key]).map_err(io_error)?;
            }
            SharedIndexOperation::SetAdmissionCount { key, uses } => {
                set_admission.execute(params![key, uses]).map_err(io_error)?;
            }
            SharedIndexOperation::RemoveAdmissionCount { key } => {
                remove_admission.execute(params![key]).map_err(io_error)?;
            }
            SharedIndexOperation::AddInvalidation { rule } => {
                let rule_json = serialize_invalidation_rule(rule)?;
                insert_invalidation.execute(params![rule_json]).map_err(io_error)?;
            }
            SharedIndexOperation::ClearInvalidations => {
                clear_invalidations.execute([]).map_err(io_error)?;
            }
        }
    }
    Ok(())
}

fn append_changes_tx(
    transaction: &Transaction<'_>,
    generation: u64,
    operations: &[SharedIndexOperation],
) -> io::Result<()> {
    let mut statement = transaction
        .prepare(
            "INSERT INTO changes (generation, op_kind, key, entry_json, uses)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .map_err(io_error)?;

    for operation in operations {
        match operation {
            SharedIndexOperation::UpsertEntry { key, entry } => {
                let entry_json = serialize_entry_record(entry)?;
                statement
                    .execute(params![
                        generation,
                        CHANGE_OP_UPSERT_ENTRY,
                        key,
                        entry_json,
                        Option::<u64>::None,
                    ])
                    .map_err(io_error)?;
            }
            SharedIndexOperation::RemoveEntry { key } => {
                statement
                    .execute(params![
                        generation,
                        CHANGE_OP_REMOVE_ENTRY,
                        key,
                        Option::<Vec<u8>>::None,
                        Option::<u64>::None,
                    ])
                    .map_err(io_error)?;
            }
            SharedIndexOperation::SetAdmissionCount { key, uses } => {
                statement
                    .execute(params![
                        generation,
                        CHANGE_OP_SET_ADMISSION_COUNT,
                        key,
                        Option::<Vec<u8>>::None,
                        Some(*uses),
                    ])
                    .map_err(io_error)?;
            }
            SharedIndexOperation::RemoveAdmissionCount { key } => {
                statement
                    .execute(params![
                        generation,
                        CHANGE_OP_REMOVE_ADMISSION_COUNT,
                        key,
                        Option::<Vec<u8>>::None,
                        Option::<u64>::None,
                    ])
                    .map_err(io_error)?;
            }
            SharedIndexOperation::AddInvalidation { rule } => {
                let rule_json = serialize_invalidation_rule(rule)?;
                statement
                    .execute(params![
                        generation,
                        CHANGE_OP_ADD_INVALIDATION,
                        "",
                        rule_json,
                        Option::<u64>::None,
                    ])
                    .map_err(io_error)?;
            }
            SharedIndexOperation::ClearInvalidations => {
                statement
                    .execute(params![
                        generation,
                        CHANGE_OP_CLEAR_INVALIDATIONS,
                        "",
                        Option::<Vec<u8>>::None,
                        Option::<u64>::None,
                    ])
                    .map_err(io_error)?;
            }
        }
    }
    Ok(())
}

fn load_changes_since_tx(
    transaction: &Transaction<'_>,
    after_seq: u64,
) -> io::Result<Vec<SharedIndexOperation>> {
    let mut statement = transaction
        .prepare(
            "SELECT op_kind, key, entry_json, uses
             FROM changes
             WHERE seq > ?1
             ORDER BY seq",
        )
        .map_err(io_error)?;
    let rows = statement
        .query_map(params![after_seq], |row| {
            let op_kind: u64 = row.get(0)?;
            let key: String = row.get(1)?;
            let entry_json: Option<Vec<u8>> = row.get(2)?;
            let uses: Option<u64> = row.get(3)?;
            Ok((op_kind, key, entry_json, uses))
        })
        .map_err(io_error)?;

    let mut operations = Vec::new();
    for row in rows {
        let (op_kind, key, entry_json, uses) = row.map_err(io_error)?;
        operations.push(change_operation_from_row(op_kind, key, entry_json, uses)?);
    }
    Ok(operations)
}

fn read_max_change_seq_tx(transaction: &Transaction<'_>) -> io::Result<u64> {
    transaction
        .query_row("SELECT COALESCE(MAX(seq), 0) FROM changes", [], |row| row.get::<_, u64>(0))
        .map_err(io_error)
}

fn change_operation_from_row(
    op_kind: u64,
    key: String,
    entry_json: Option<Vec<u8>>,
    uses: Option<u64>,
) -> io::Result<SharedIndexOperation> {
    match op_kind {
        CHANGE_OP_UPSERT_ENTRY => Ok(SharedIndexOperation::UpsertEntry {
            key,
            entry: deserialize_entry_record(entry_json.as_deref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "missing delta entry payload")
            })?)?,
        }),
        CHANGE_OP_REMOVE_ENTRY => Ok(SharedIndexOperation::RemoveEntry { key }),
        CHANGE_OP_SET_ADMISSION_COUNT => Ok(SharedIndexOperation::SetAdmissionCount {
            key,
            uses: uses.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "missing delta admission count payload")
            })?,
        }),
        CHANGE_OP_REMOVE_ADMISSION_COUNT => Ok(SharedIndexOperation::RemoveAdmissionCount { key }),
        CHANGE_OP_ADD_INVALIDATION => Ok(SharedIndexOperation::AddInvalidation {
            rule: deserialize_invalidation_rule(entry_json.as_deref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "missing delta invalidation payload")
            })?)?,
        }),
        CHANGE_OP_CLEAR_INVALIDATIONS => Ok(SharedIndexOperation::ClearInvalidations),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown shared index delta op kind `{other}`"),
        )),
    }
}

fn load_entries_into_index(
    transaction: &Transaction<'_>,
    index: &mut CacheIndex,
) -> io::Result<()> {
    let mut statement = transaction
        .prepare("SELECT key, entry_json FROM entries ORDER BY key")
        .map_err(io_error)?;
    let rows = statement
        .query_map([], |row| {
            let key: String = row.get(0)?;
            let entry_json: Vec<u8> = row.get(1)?;
            Ok((key, entry_json))
        })
        .map_err(io_error)?;

    for row in rows {
        let (key, entry_json) = row.map_err(io_error)?;
        let entry = deserialize_entry_record(&entry_json)?;
        if let Some(existing) = index.insert_entry(key.clone(), entry.clone()) {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(existing.body_size_bytes);
            if let Some(keys) = index.variants.get_mut(&existing.base_key) {
                keys.retain(|candidate| candidate != &key);
            }
            if index.variants.get(&existing.base_key).is_some_and(|keys| keys.is_empty()) {
                index.variants.remove(&existing.base_key);
            }
        }
        index.current_size_bytes = index.current_size_bytes.saturating_add(entry.body_size_bytes);
        let variant_keys = index.variants.entry(entry.base_key.clone()).or_default();
        if !variant_keys.contains(&key) {
            variant_keys.push(key);
        }
    }
    Ok(())
}

fn load_admission_counts_into_index(
    transaction: &Transaction<'_>,
    index: &mut CacheIndex,
) -> io::Result<()> {
    let mut statement = transaction
        .prepare("SELECT key, uses FROM admission_counts ORDER BY key")
        .map_err(io_error)?;
    let rows = statement
        .query_map([], |row| {
            let key: String = row.get(0)?;
            let uses: u64 = row.get(1)?;
            Ok((key, uses))
        })
        .map_err(io_error)?;
    for row in rows {
        let (key, uses) = row.map_err(io_error)?;
        index.admission_counts.insert(key, uses);
    }
    Ok(())
}

fn load_invalidations_into_index(
    transaction: &Transaction<'_>,
    index: &mut CacheIndex,
) -> io::Result<()> {
    let mut statement = transaction
        .prepare("SELECT rule_json FROM invalidations ORDER BY seq")
        .map_err(io_error)?;
    let rows = statement
        .query_map([], |row| {
            let rule_json: Vec<u8> = row.get(0)?;
            Ok(rule_json)
        })
        .map_err(io_error)?;
    for row in rows {
        let rule_json = row.map_err(io_error)?;
        index.invalidations.push(deserialize_invalidation_rule(&rule_json)?);
    }
    Ok(())
}

fn clear_index_tables(transaction: &Transaction<'_>) -> io::Result<()> {
    transaction.execute("DELETE FROM entries", []).map_err(io_error)?;
    transaction.execute("DELETE FROM admission_counts", []).map_err(io_error)?;
    transaction.execute("DELETE FROM invalidations", []).map_err(io_error)?;
    transaction.execute("DELETE FROM changes", []).map_err(io_error)?;
    Ok(())
}

fn insert_entries(transaction: &Transaction<'_>, index: &CacheIndex) -> io::Result<()> {
    let mut statement = transaction
        .prepare("INSERT INTO entries (key, entry_json) VALUES (?1, ?2)")
        .map_err(io_error)?;
    for (key, entry) in &index.entries {
        let entry_json = serialize_entry_record(entry)?;
        statement.execute(params![key, entry_json]).map_err(io_error)?;
    }
    Ok(())
}

fn insert_admission_counts(transaction: &Transaction<'_>, index: &CacheIndex) -> io::Result<()> {
    let mut statement = transaction
        .prepare("INSERT INTO admission_counts (key, uses) VALUES (?1, ?2)")
        .map_err(io_error)?;
    for (key, uses) in &index.admission_counts {
        statement.execute(params![key, uses]).map_err(io_error)?;
    }
    Ok(())
}

fn insert_invalidations(transaction: &Transaction<'_>, index: &CacheIndex) -> io::Result<()> {
    let mut statement = transaction
        .prepare("INSERT INTO invalidations (rule_json) VALUES (?1)")
        .map_err(io_error)?;
    for rule in &index.invalidations {
        let rule_json = serialize_invalidation_rule(rule)?;
        statement.execute(params![rule_json]).map_err(io_error)?;
    }
    Ok(())
}
