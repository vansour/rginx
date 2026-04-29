use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::super::entry::cache_key_hash;
use super::super::{CacheIndex, CacheIndexEntry, CachedVaryHeaderValue};
use super::SharedIndexOperation;

const SHARED_INDEX_SCHEMA_VERSION: u64 = 1;
const SHARED_INDEX_FILENAME: &str = ".rginx-index.sqlite3";
const LEGACY_SHARED_INDEX_FILENAME: &str = ".rginx-index.json";
const SHARED_FILL_LOCK_PREFIX: &str = ".rginx-fill-";
const SHARED_FILL_LOCK_SUFFIX: &str = ".lock";
const META_SCHEMA_VERSION_KEY: &str = "schema_version";
const META_GENERATION_KEY: &str = "generation";

pub(in crate::cache) struct SharedIndexStore {
    path: PathBuf,
}

pub(super) struct LoadedSharedIndex {
    pub(super) index: CacheIndex,
    pub(super) generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SharedIndexEntryRecord {
    hash: String,
    base_key: String,
    vary: Vec<SharedVaryHeader>,
    body_size_bytes: usize,
    expires_at_unix_ms: u64,
    #[serde(default)]
    stale_if_error_until_unix_ms: Option<u64>,
    #[serde(default)]
    stale_while_revalidate_until_unix_ms: Option<u64>,
    #[serde(default)]
    must_revalidate: bool,
    last_access_unix_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SharedVaryHeader {
    name: String,
    #[serde(default)]
    value: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LegacySharedIndexFile {
    version: u8,
    generation: u64,
    entries: Vec<LegacySharedIndexEntry>,
    #[serde(default)]
    admission_counts: Vec<LegacySharedAdmissionCount>,
}

#[derive(Debug, Deserialize)]
struct LegacySharedIndexEntry {
    key: String,
    hash: String,
    base_key: String,
    vary: Vec<SharedVaryHeader>,
    body_size_bytes: usize,
    expires_at_unix_ms: u64,
    #[serde(default)]
    stale_if_error_until_unix_ms: Option<u64>,
    #[serde(default)]
    stale_while_revalidate_until_unix_ms: Option<u64>,
    #[serde(default)]
    must_revalidate: bool,
    last_access_unix_ms: u64,
}

#[derive(Debug, Deserialize)]
struct LegacySharedAdmissionCount {
    key: String,
    uses: u64,
}

pub(in crate::cache) fn shared_fill_lock_path(zone: &rginx_core::CacheZone, key: &str) -> PathBuf {
    zone.path
        .join(format!("{SHARED_FILL_LOCK_PREFIX}{}{SHARED_FILL_LOCK_SUFFIX}", cache_key_hash(key)))
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
    let file: LegacySharedIndexFile = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if file.version != SHARED_INDEX_SCHEMA_VERSION as u8 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unsupported legacy shared index version `{}` in `{}`",
                file.version,
                path.display()
            ),
        ));
    }

    Ok(Some(index_from_legacy_shared_file(file)))
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
    SharedIndexStore { path: shared_index_path(zone) }
}

pub(super) fn load_shared_index_from_disk(
    store: &SharedIndexStore,
) -> io::Result<LoadedSharedIndex> {
    let connection = store.open_connection()?;
    let generation = read_meta_u64(&connection, META_GENERATION_KEY)?.unwrap_or_default();

    let mut index = CacheIndex::default();
    let mut statement =
        connection.prepare("SELECT key, entry_json FROM entries ORDER BY key").map_err(io_error)?;
    let rows = statement
        .query_map([], |row| {
            let key: String = row.get(0)?;
            let entry_json: Vec<u8> = row.get(1)?;
            Ok((key, entry_json))
        })
        .map_err(io_error)?;

    for row in rows {
        let (key, entry_json) = row.map_err(io_error)?;
        let record: SharedIndexEntryRecord =
            serde_json::from_slice(&entry_json).map_err(invalid_data_error)?;
        let entry = cache_index_entry_from_record(record)?;
        index.current_size_bytes = index.current_size_bytes.saturating_add(entry.body_size_bytes);
        index.variants.entry(entry.base_key.clone()).or_default().push(key.clone());
        index.entries.insert(key, entry);
    }

    let mut statement = connection
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

    Ok(LoadedSharedIndex { index, generation })
}

pub(super) fn recreate_shared_index_on_disk(
    store: &SharedIndexStore,
    index: &CacheIndex,
    generation: u64,
) -> io::Result<u64> {
    store.recreate(index, generation)
}

pub(super) fn shared_index_generation(store: &SharedIndexStore) -> io::Result<u64> {
    store.generation()
}

pub(super) fn apply_shared_index_operations(
    store: &SharedIndexStore,
    operations: &[SharedIndexOperation],
) -> io::Result<u64> {
    store.apply_operations(operations)
}

pub(super) fn run_blocking<T>(operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    if let Ok(handle) = tokio::runtime::Handle::try_current()
        && handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
    {
        return tokio::task::block_in_place(operation);
    }
    operation()
}

impl SharedIndexStore {
    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    fn generation(&self) -> io::Result<u64> {
        let connection = self.open_connection()?;
        Ok(read_meta_u64(&connection, META_GENERATION_KEY)?.unwrap_or_default())
    }

    fn replace_all(&self, index: &CacheIndex, generation: u64) -> io::Result<u64> {
        let mut connection = self.open_connection()?;
        let transaction = connection.transaction().map_err(io_error)?;
        transaction.execute("DELETE FROM entries", []).map_err(io_error)?;
        transaction.execute("DELETE FROM admission_counts", []).map_err(io_error)?;

        {
            let mut statement = transaction
                .prepare("INSERT INTO entries (key, entry_json) VALUES (?1, ?2)")
                .map_err(io_error)?;
            for (key, entry) in &index.entries {
                let entry_json =
                    serde_json::to_vec(&record_from_cache_index_entry(entry)).map_err(io_error)?;
                statement.execute(params![key, entry_json]).map_err(io_error)?;
            }
        }

        {
            let mut statement = transaction
                .prepare("INSERT INTO admission_counts (key, uses) VALUES (?1, ?2)")
                .map_err(io_error)?;
            for (key, uses) in &index.admission_counts {
                statement.execute(params![key, uses]).map_err(io_error)?;
            }
        }

        write_meta_u64(&transaction, META_GENERATION_KEY, generation)?;
        transaction.commit().map_err(io_error)?;
        Ok(generation)
    }

    fn recreate(&self, index: &CacheIndex, generation: u64) -> io::Result<u64> {
        match self.open_raw_connection() {
            Ok(connection) => {
                drop_schema(&connection)?;
            }
            Err(_) => {
                let _ = fs::remove_file(&self.path);
            }
        }
        self.replace_all(index, generation)
    }

    fn apply_operations(&self, operations: &[SharedIndexOperation]) -> io::Result<u64> {
        if operations.is_empty() {
            return self.generation();
        }

        let mut connection = self.open_connection()?;
        let transaction = connection.transaction().map_err(io_error)?;
        let current_generation =
            read_meta_u64_tx(&transaction, META_GENERATION_KEY)?.unwrap_or_default();

        {
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
            let mut remove_admission = transaction
                .prepare("DELETE FROM admission_counts WHERE key = ?1")
                .map_err(io_error)?;

            for operation in operations {
                match operation {
                    SharedIndexOperation::UpsertEntry { key, entry } => {
                        let entry_json = serde_json::to_vec(&record_from_cache_index_entry(entry))
                            .map_err(io_error)?;
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
        }

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

fn initialize_schema(connection: &Connection) -> io::Result<()> {
    connection.pragma_update(None, "journal_mode", "WAL").map_err(io_error)?;
    connection.pragma_update(None, "synchronous", "NORMAL").map_err(io_error)?;
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS metadata (
                key TEXT PRIMARY KEY NOT NULL,
                int_value INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS entries (
                key TEXT PRIMARY KEY NOT NULL,
                entry_json BLOB NOT NULL
            );
            CREATE TABLE IF NOT EXISTS admission_counts (
                key TEXT PRIMARY KEY NOT NULL,
                uses INTEGER NOT NULL
            );",
        )
        .map_err(io_error)?;
    connection
        .execute(
            "INSERT OR IGNORE INTO metadata (key, int_value) VALUES (?1, ?2)",
            params![META_SCHEMA_VERSION_KEY, SHARED_INDEX_SCHEMA_VERSION],
        )
        .map_err(io_error)?;
    connection
        .execute(
            "INSERT OR IGNORE INTO metadata (key, int_value) VALUES (?1, 0)",
            params![META_GENERATION_KEY],
        )
        .map_err(io_error)?;

    let schema_version = read_meta_u64(connection, META_SCHEMA_VERSION_KEY)?.unwrap_or_default();
    if schema_version != SHARED_INDEX_SCHEMA_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported shared index schema version `{schema_version}`"),
        ));
    }

    Ok(())
}

fn drop_schema(connection: &Connection) -> io::Result<()> {
    connection
        .execute_batch(
            "DROP TABLE IF EXISTS admission_counts;
             DROP TABLE IF EXISTS entries;
             DROP TABLE IF EXISTS metadata;",
        )
        .map_err(io_error)
}

fn read_meta_u64(connection: &Connection, key: &str) -> io::Result<Option<u64>> {
    connection
        .query_row("SELECT int_value FROM metadata WHERE key = ?1", params![key], |row| {
            row.get::<_, u64>(0)
        })
        .optional()
        .map_err(io_error)
}

fn read_meta_u64_tx(transaction: &rusqlite::Transaction<'_>, key: &str) -> io::Result<Option<u64>> {
    transaction
        .query_row("SELECT int_value FROM metadata WHERE key = ?1", params![key], |row| {
            row.get::<_, u64>(0)
        })
        .optional()
        .map_err(io_error)
}

fn write_meta_u64(
    transaction: &rusqlite::Transaction<'_>,
    key: &str,
    value: u64,
) -> io::Result<()> {
    transaction
        .execute(
            "INSERT INTO metadata (key, int_value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET int_value = excluded.int_value",
            params![key, value],
        )
        .map_err(io_error)?;
    Ok(())
}

fn record_from_cache_index_entry(entry: &CacheIndexEntry) -> SharedIndexEntryRecord {
    SharedIndexEntryRecord {
        hash: entry.hash.clone(),
        base_key: entry.base_key.clone(),
        vary: entry
            .vary
            .iter()
            .map(|header| SharedVaryHeader {
                name: header.name.as_str().to_string(),
                value: header.value.clone(),
            })
            .collect(),
        body_size_bytes: entry.body_size_bytes,
        expires_at_unix_ms: entry.expires_at_unix_ms,
        stale_if_error_until_unix_ms: entry.stale_if_error_until_unix_ms,
        stale_while_revalidate_until_unix_ms: entry.stale_while_revalidate_until_unix_ms,
        must_revalidate: entry.must_revalidate,
        last_access_unix_ms: entry.last_access_unix_ms,
    }
}

fn cache_index_entry_from_record(record: SharedIndexEntryRecord) -> io::Result<CacheIndexEntry> {
    let vary = record
        .vary
        .into_iter()
        .map(|header| {
            Ok(CachedVaryHeaderValue {
                name: header
                    .name
                    .parse::<http::header::HeaderName>()
                    .map_err(invalid_data_error)?,
                value: header.value,
            })
        })
        .collect::<io::Result<Vec<_>>>()?;

    Ok(CacheIndexEntry {
        hash: record.hash,
        base_key: record.base_key,
        vary,
        body_size_bytes: record.body_size_bytes,
        expires_at_unix_ms: record.expires_at_unix_ms,
        stale_if_error_until_unix_ms: record.stale_if_error_until_unix_ms,
        stale_while_revalidate_until_unix_ms: record.stale_while_revalidate_until_unix_ms,
        must_revalidate: record.must_revalidate,
        last_access_unix_ms: record.last_access_unix_ms,
    })
}

fn index_from_legacy_shared_file(file: LegacySharedIndexFile) -> LoadedSharedIndex {
    let mut index = CacheIndex::default();
    for entry in file.entries {
        let vary = entry
            .vary
            .into_iter()
            .filter_map(|header| {
                Some(CachedVaryHeaderValue {
                    name: header.name.parse::<http::header::HeaderName>().ok()?,
                    value: header.value,
                })
            })
            .collect::<Vec<_>>();
        let key = entry.key;
        let index_entry = CacheIndexEntry {
            hash: entry.hash,
            base_key: entry.base_key.clone(),
            vary,
            body_size_bytes: entry.body_size_bytes,
            expires_at_unix_ms: entry.expires_at_unix_ms,
            stale_if_error_until_unix_ms: entry.stale_if_error_until_unix_ms,
            stale_while_revalidate_until_unix_ms: entry.stale_while_revalidate_until_unix_ms,
            must_revalidate: entry.must_revalidate,
            last_access_unix_ms: entry.last_access_unix_ms,
        };
        index.current_size_bytes =
            index.current_size_bytes.saturating_add(index_entry.body_size_bytes);
        index.variants.entry(entry.base_key).or_default().push(key.clone());
        index.entries.insert(key, index_entry);
    }
    for admission in file.admission_counts {
        index.admission_counts.insert(admission.key, admission.uses);
    }
    LoadedSharedIndex { index, generation: file.generation }
}

fn io_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::other(error.to_string())
}

fn invalid_data_error(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
