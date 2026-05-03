use std::io;

use super::super::SharedIndexOperation;
use super::super::codec::{
    deserialize_entry_record, deserialize_invalidation_rule, serialize_entry_record,
    serialize_invalidation_rule,
};
use super::document::{SharedMemoryChangeRecord, SharedMemoryIndexDocument};

const CHANGE_OP_UPSERT_ENTRY: u8 = 1;
const CHANGE_OP_REMOVE_ENTRY: u8 = 2;
const CHANGE_OP_SET_ADMISSION_COUNT: u8 = 3;
const CHANGE_OP_REMOVE_ADMISSION_COUNT: u8 = 4;
const CHANGE_OP_ADD_INVALIDATION: u8 = 5;
const CHANGE_OP_CLEAR_INVALIDATIONS: u8 = 6;
const CHANGE_OP_TOUCH_ENTRY: u8 = 7;

pub(super) fn apply_operation_to_document(
    document: &mut SharedMemoryIndexDocument,
    operation: &SharedIndexOperation,
) -> io::Result<()> {
    match operation {
        SharedIndexOperation::UpsertEntry { key, entry } => {
            document.entries.insert(key.clone(), serialize_entry_record(entry)?);
        }
        SharedIndexOperation::RemoveEntry { key } => {
            document.entries.remove(key);
        }
        SharedIndexOperation::TouchEntry { key, last_access_unix_ms } => {
            let Some(entry_json) = document.entries.get_mut(key) else {
                return Ok(());
            };
            let mut entry = deserialize_entry_record(entry_json)?;
            if entry.last_access_unix_ms < *last_access_unix_ms {
                entry.last_access_unix_ms = *last_access_unix_ms;
                *entry_json = serialize_entry_record(&entry)?;
            }
        }
        SharedIndexOperation::SetAdmissionCount { key, uses } => {
            document.admission_counts.insert(key.clone(), *uses);
        }
        SharedIndexOperation::RemoveAdmissionCount { key } => {
            document.admission_counts.remove(key);
        }
        SharedIndexOperation::AddInvalidation { rule } => {
            document.invalidations.push(serialize_invalidation_rule(rule)?);
        }
        SharedIndexOperation::ClearInvalidations => {
            document.invalidations.clear();
        }
    }
    Ok(())
}

pub(super) fn change_record_from_operation(
    seq: u64,
    generation: u64,
    operation: &SharedIndexOperation,
) -> io::Result<SharedMemoryChangeRecord> {
    match operation {
        SharedIndexOperation::UpsertEntry { key, entry } => Ok(SharedMemoryChangeRecord {
            seq,
            generation,
            op_kind: CHANGE_OP_UPSERT_ENTRY,
            key: key.clone(),
            entry_json: Some(serialize_entry_record(entry)?),
            uses: None,
            last_access_unix_ms: None,
        }),
        SharedIndexOperation::RemoveEntry { key } => Ok(SharedMemoryChangeRecord {
            seq,
            generation,
            op_kind: CHANGE_OP_REMOVE_ENTRY,
            key: key.clone(),
            entry_json: None,
            uses: None,
            last_access_unix_ms: None,
        }),
        SharedIndexOperation::TouchEntry { key, last_access_unix_ms } => {
            Ok(SharedMemoryChangeRecord {
                seq,
                generation,
                op_kind: CHANGE_OP_TOUCH_ENTRY,
                key: key.clone(),
                entry_json: None,
                uses: None,
                last_access_unix_ms: Some(*last_access_unix_ms),
            })
        }
        SharedIndexOperation::SetAdmissionCount { key, uses } => Ok(SharedMemoryChangeRecord {
            seq,
            generation,
            op_kind: CHANGE_OP_SET_ADMISSION_COUNT,
            key: key.clone(),
            entry_json: None,
            uses: Some(*uses),
            last_access_unix_ms: None,
        }),
        SharedIndexOperation::RemoveAdmissionCount { key } => Ok(SharedMemoryChangeRecord {
            seq,
            generation,
            op_kind: CHANGE_OP_REMOVE_ADMISSION_COUNT,
            key: key.clone(),
            entry_json: None,
            uses: None,
            last_access_unix_ms: None,
        }),
        SharedIndexOperation::AddInvalidation { rule } => Ok(SharedMemoryChangeRecord {
            seq,
            generation,
            op_kind: CHANGE_OP_ADD_INVALIDATION,
            key: String::new(),
            entry_json: Some(serialize_invalidation_rule(rule)?),
            uses: None,
            last_access_unix_ms: None,
        }),
        SharedIndexOperation::ClearInvalidations => Ok(SharedMemoryChangeRecord {
            seq,
            generation,
            op_kind: CHANGE_OP_CLEAR_INVALIDATIONS,
            key: String::new(),
            entry_json: None,
            uses: None,
            last_access_unix_ms: None,
        }),
    }
}

pub(super) fn operations_since(
    document: &SharedMemoryIndexDocument,
    after_seq: u64,
) -> io::Result<Vec<SharedIndexOperation>> {
    let Some(first_change) = document.changes.first() else {
        return Ok(Vec::new());
    };
    // A gap means the local cursor fell behind the bounded ring; callers
    // treat an empty delta as the signal to perform a full reload.
    if after_seq < first_change.seq.saturating_sub(1) {
        return Ok(Vec::new());
    }
    document
        .changes
        .iter()
        .filter(|change| change.seq > after_seq)
        .map(operation_from_change_record)
        .collect()
}

fn operation_from_change_record(
    record: &SharedMemoryChangeRecord,
) -> io::Result<SharedIndexOperation> {
    match record.op_kind {
        CHANGE_OP_UPSERT_ENTRY => Ok(SharedIndexOperation::UpsertEntry {
            key: record.key.clone(),
            entry: deserialize_entry_record(record.entry_json.as_deref().ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "missing shm delta entry payload")
            })?)?,
        }),
        CHANGE_OP_REMOVE_ENTRY => Ok(SharedIndexOperation::RemoveEntry { key: record.key.clone() }),
        CHANGE_OP_TOUCH_ENTRY => Ok(SharedIndexOperation::TouchEntry {
            key: record.key.clone(),
            last_access_unix_ms: record.last_access_unix_ms.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "missing shm delta touch payload")
            })?,
        }),
        CHANGE_OP_SET_ADMISSION_COUNT => Ok(SharedIndexOperation::SetAdmissionCount {
            key: record.key.clone(),
            uses: record.uses.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "missing shm delta admission payload")
            })?,
        }),
        CHANGE_OP_REMOVE_ADMISSION_COUNT => {
            Ok(SharedIndexOperation::RemoveAdmissionCount { key: record.key.clone() })
        }
        CHANGE_OP_ADD_INVALIDATION => Ok(SharedIndexOperation::AddInvalidation {
            rule: deserialize_invalidation_rule(record.entry_json.as_deref().ok_or_else(
                || {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "missing shm delta invalidation payload",
                    )
                },
            )?)?,
        }),
        CHANGE_OP_CLEAR_INVALIDATIONS => Ok(SharedIndexOperation::ClearInvalidations),
        other => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unknown shared memory delta op kind `{other}`"),
        )),
    }
}

pub(super) fn trim_change_ring(
    document: &mut SharedMemoryIndexDocument,
    operation_ring_capacity: usize,
) {
    if operation_ring_capacity == 0 {
        document.changes.clear();
        return;
    }
    if document.changes.len() > operation_ring_capacity {
        let drop_count = document.changes.len() - operation_ring_capacity;
        document.changes.drain(0..drop_count);
    }
}
