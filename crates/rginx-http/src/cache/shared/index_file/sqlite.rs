use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use super::super::super::CacheIndex;
use super::super::SharedIndexOperation;
use super::codec::{deserialize_entry_record, serialize_entry_record};
use super::schema::{
    initialize_schema, read_meta_u64, read_meta_u64_tx, reset_schema, write_meta_u64,
};
use super::{LoadedSharedIndex, META_GENERATION_KEY, io_error};

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
        let mut index = CacheIndex::default();
        load_entries_into_index(&transaction, &mut index)?;
        load_admission_counts_into_index(&transaction, &mut index)?;
        Ok(LoadedSharedIndex { index, generation })
    }

    pub(super) fn generation(&self) -> io::Result<u64> {
        let connection = self.open_connection()?;
        Ok(read_meta_u64(&connection, META_GENERATION_KEY)?.unwrap_or_default())
    }

    pub(super) fn recreate(&self, index: &CacheIndex, generation: u64) -> io::Result<u64> {
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

    pub(super) fn apply_operations(&self, operations: &[SharedIndexOperation]) -> io::Result<u64> {
        if operations.is_empty() {
            return self.generation();
        }

        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(io_error)?;
        let current_generation =
            read_meta_u64_tx(&transaction, META_GENERATION_KEY)?.unwrap_or_default();
        apply_operations_tx(&transaction, operations)?;
        let next_generation = current_generation.saturating_add(1);
        write_meta_u64(&transaction, META_GENERATION_KEY, next_generation)?;
        transaction.commit().map_err(io_error)?;
        Ok(next_generation)
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
) -> io::Result<u64> {
    let transaction =
        connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(io_error)?;
    clear_index_tables(&transaction)?;
    insert_entries(&transaction, index)?;
    insert_admission_counts(&transaction, index)?;
    write_meta_u64(&transaction, META_GENERATION_KEY, generation)?;
    transaction.commit().map_err(io_error)?;
    Ok(generation)
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
        }
    }
    Ok(())
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
        index.current_size_bytes = index.current_size_bytes.saturating_add(entry.body_size_bytes);
        index.variants.entry(entry.base_key.clone()).or_default().push(key.clone());
        index.entries.insert(key, entry);
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

fn clear_index_tables(transaction: &Transaction<'_>) -> io::Result<()> {
    transaction.execute("DELETE FROM entries", []).map_err(io_error)?;
    transaction.execute("DELETE FROM admission_counts", []).map_err(io_error)?;
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
