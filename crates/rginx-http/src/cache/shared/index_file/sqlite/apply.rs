use super::*;

pub(super) fn replace_all(
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

pub(super) fn apply_operations_tx(
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

pub(super) fn append_changes_tx(
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
