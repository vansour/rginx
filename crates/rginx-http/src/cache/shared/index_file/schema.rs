use std::io;

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use super::{META_GENERATION_KEY, META_SCHEMA_VERSION_KEY, SHARED_INDEX_SCHEMA_VERSION, io_error};

const CREATE_SCHEMA_SQL: &str = "CREATE TABLE IF NOT EXISTS metadata (
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
    );";
const DROP_SCHEMA_SQL: &str = "DROP TABLE IF EXISTS admission_counts;
    DROP TABLE IF EXISTS entries;
    DROP TABLE IF EXISTS metadata;";

pub(super) fn initialize_schema(connection: &Connection) -> io::Result<()> {
    connection.pragma_update(None, "journal_mode", "WAL").map_err(io_error)?;
    connection.pragma_update(None, "synchronous", "NORMAL").map_err(io_error)?;
    connection.execute_batch(CREATE_SCHEMA_SQL).map_err(io_error)?;
    seed_metadata_connection(connection)?;
    validate_schema(connection)
}

pub(super) fn reset_schema(connection: &mut Connection) -> io::Result<()> {
    let transaction =
        connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(io_error)?;
    transaction.execute_batch(DROP_SCHEMA_SQL).map_err(io_error)?;
    transaction.execute_batch(CREATE_SCHEMA_SQL).map_err(io_error)?;
    seed_metadata_tx(&transaction)?;
    transaction.commit().map_err(io_error)?;
    Ok(())
}

pub(super) fn read_meta_u64(connection: &Connection, key: &str) -> io::Result<Option<u64>> {
    connection
        .query_row("SELECT int_value FROM metadata WHERE key = ?1", params![key], |row| {
            row.get::<_, u64>(0)
        })
        .optional()
        .map_err(io_error)
}

pub(super) fn read_meta_u64_tx(
    transaction: &Transaction<'_>,
    key: &str,
) -> io::Result<Option<u64>> {
    transaction
        .query_row("SELECT int_value FROM metadata WHERE key = ?1", params![key], |row| {
            row.get::<_, u64>(0)
        })
        .optional()
        .map_err(io_error)
}

pub(super) fn write_meta_u64(
    transaction: &Transaction<'_>,
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

fn validate_schema(connection: &Connection) -> io::Result<()> {
    let schema_version = read_meta_u64(connection, META_SCHEMA_VERSION_KEY)?.unwrap_or_default();
    if schema_version != SHARED_INDEX_SCHEMA_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported shared index schema version `{schema_version}`"),
        ));
    }
    Ok(())
}

fn seed_metadata_connection(connection: &Connection) -> io::Result<()> {
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
    Ok(())
}

fn seed_metadata_tx(transaction: &Transaction<'_>) -> io::Result<()> {
    transaction
        .execute(
            "INSERT OR IGNORE INTO metadata (key, int_value) VALUES (?1, ?2)",
            params![META_SCHEMA_VERSION_KEY, SHARED_INDEX_SCHEMA_VERSION],
        )
        .map_err(io_error)?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO metadata (key, int_value) VALUES (?1, 0)",
            params![META_GENERATION_KEY],
        )
        .map_err(io_error)?;
    Ok(())
}
