use super::*;

pub(super) fn load_changes_since_tx(
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

pub(super) fn read_max_change_seq_tx(transaction: &Transaction<'_>) -> io::Result<u64> {
    transaction
        .query_row("SELECT COALESCE(MAX(seq), 0) FROM changes", [], |row| row.get::<_, u64>(0))
        .map_err(io_error)
}

pub(super) fn load_entries_into_index(
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

pub(super) fn load_admission_counts_into_index(
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

pub(super) fn load_invalidations_into_index(
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
