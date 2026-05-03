use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use super::codec::{
    deserialize_entry_record, deserialize_invalidation_rule, serialize_entry_record,
    serialize_invalidation_rule,
};
use super::{
    AppliedSharedIndexOperations, LoadedSharedIndex, LoadedSharedIndexChanges,
    SharedFillLockAcquire, SharedFillLockSnapshot, SharedFillLockStatus, SharedIndexBackend,
    SharedIndexMetrics, SharedIndexOperation, SharedIndexSyncState, invalid_data_error,
};
use crate::cache::CacheIndex;
use crate::cache::entry::cache_key_hash;
use crate::cache::shared::memory::{SharedMemorySegment, SharedMemorySegmentConfig};

const PAYLOAD_LEN_BYTES: usize = 8;
const DEFAULT_SHM_CAPACITY_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_OPERATION_RING_CAPACITY: u64 = 4_096;
const CHANGE_OP_UPSERT_ENTRY: u8 = 1;
const CHANGE_OP_REMOVE_ENTRY: u8 = 2;
const CHANGE_OP_SET_ADMISSION_COUNT: u8 = 3;
const CHANGE_OP_REMOVE_ADMISSION_COUNT: u8 = 4;
const CHANGE_OP_ADD_INVALIDATION: u8 = 5;
const CHANGE_OP_CLEAR_INVALIDATIONS: u8 = 6;
const CHANGE_OP_TOUCH_ENTRY: u8 = 7;

pub(super) struct MemorySharedIndexStore {
    path: PathBuf,
    lock_path: PathBuf,
    segment_config: SharedMemorySegmentConfig,
    operation_ring_capacity: usize,
    lock_contention_total: AtomicU64,
    full_reload_total: AtomicU64,
    capacity_rejection_total: AtomicU64,
}

#[derive(Debug, Serialize, Deserialize)]
struct SharedMemoryIndexDocument {
    version: u32,
    generation: u64,
    store_epoch: u64,
    last_change_seq: u64,
    #[serde(default)]
    rebuild_total: u64,
    #[serde(default)]
    stale_fill_lock_cleanup_total: u64,
    entries: BTreeMap<String, Vec<u8>>,
    admission_counts: BTreeMap<String, u64>,
    invalidations: Vec<Vec<u8>>,
    #[serde(default)]
    fill_locks: BTreeMap<String, SharedMemoryFillLockRecord>,
    changes: Vec<SharedMemoryChangeRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SharedMemoryChangeRecord {
    seq: u64,
    generation: u64,
    op_kind: u8,
    key: String,
    entry_json: Option<Vec<u8>>,
    uses: Option<u64>,
    #[serde(default)]
    last_access_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SharedMemoryFillLockRecord {
    key_hash: String,
    owner_pid: u32,
    owner_generation: u64,
    nonce: String,
    acquired_at_unix_ms: u64,
    updated_at_unix_ms: u64,
    #[serde(default)]
    released: bool,
    state_json: Vec<u8>,
}

struct FileLock {
    file: File,
}

impl MemorySharedIndexStore {
    pub(super) fn new(zone: &rginx_core::CacheZone) -> Self {
        let capacity_bytes = memory_capacity_bytes();
        let operation_ring_capacity = memory_operation_ring_capacity();
        let identity = format!("{}:{}", zone.name, zone.path.display());
        let segment_config = SharedMemorySegmentConfig::for_identity(&identity, capacity_bytes)
            .with_operation_ring_capacity(operation_ring_capacity as u64);
        Self {
            path: zone.path.join(".rginx-index.shm"),
            lock_path: zone.path.join(".rginx-index.shm.lock"),
            segment_config,
            operation_ring_capacity,
            lock_contention_total: AtomicU64::new(0),
            full_reload_total: AtomicU64::new(0),
            capacity_rejection_total: AtomicU64::new(0),
        }
    }

    fn with_document_lock<T>(
        &self,
        operation: impl FnOnce(&SharedMemorySegment, SharedMemoryIndexDocument) -> io::Result<T>,
    ) -> io::Result<T> {
        let _lock = self.lock()?;
        let segment = self.open_or_create_segment()?;
        let document = self.read_document(&segment)?;
        operation(&segment, document)
    }

    fn lock(&self) -> io::Result<FileLock> {
        if let Some(parent) = self.lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.lock_path)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(FileLock { file });
        }

        let error = io::Error::last_os_error();
        let would_block = error.kind() == io::ErrorKind::WouldBlock
            || error.raw_os_error().is_some_and(|code| code == libc::EWOULDBLOCK);
        if !would_block {
            return Err(error);
        }

        self.lock_contention_total.fetch_add(1, Ordering::Relaxed);
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            return Ok(FileLock { file });
        }
        Err(io::Error::last_os_error())
    }

    fn open_or_create_segment(&self) -> io::Result<SharedMemorySegment> {
        match SharedMemorySegment::attach(&self.segment_config) {
            Ok(segment) => Ok(segment),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let segment = SharedMemorySegment::create_or_reset(&self.segment_config)?;
                self.write_document(
                    &segment,
                    &SharedMemoryIndexDocument::empty(segment.header().store_epoch),
                )?;
                Ok(segment)
            }
            Err(error) if error.kind() == io::ErrorKind::InvalidData => {
                let _ = SharedMemorySegment::unlink(&self.segment_config);
                let segment = SharedMemorySegment::create_or_reset(&self.segment_config)?;
                self.write_document(
                    &segment,
                    &SharedMemoryIndexDocument::empty(segment.header().store_epoch),
                )?;
                Ok(segment)
            }
            Err(error) => Err(error),
        }
    }

    fn read_document(
        &self,
        segment: &SharedMemorySegment,
    ) -> io::Result<SharedMemoryIndexDocument> {
        let payload_capacity = segment.payload_capacity();
        if payload_capacity < PAYLOAD_LEN_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "shared memory payload is too small for document length",
            ));
        }
        let len_bytes = segment.read_payload(0, PAYLOAD_LEN_BYTES)?;
        let len = u64::from_le_bytes(
            len_bytes
                .as_slice()
                .try_into()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        );
        if len == 0 {
            return Ok(SharedMemoryIndexDocument::empty(segment.header().store_epoch));
        }
        let len = usize::try_from(len).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "shared memory document length is too large")
        })?;
        let document_capacity = payload_capacity.saturating_sub(PAYLOAD_LEN_BYTES);
        if len > document_capacity {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "shared memory document length exceeds payload capacity: length {len}, capacity {document_capacity}"
                ),
            ));
        }
        let bytes = segment.read_payload(PAYLOAD_LEN_BYTES, len)?;
        let document: SharedMemoryIndexDocument =
            serde_json::from_slice(&bytes).map_err(invalid_data_error)?;
        if document.version != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unsupported shared memory index document version `{}`", document.version),
            ));
        }
        Ok(document)
    }

    fn write_document(
        &self,
        segment: &SharedMemorySegment,
        document: &SharedMemoryIndexDocument,
    ) -> io::Result<()> {
        let bytes = serde_json::to_vec(&document).map_err(invalid_data_error)?;
        let document_capacity = segment.payload_capacity().saturating_sub(PAYLOAD_LEN_BYTES);
        if bytes.len() > document_capacity {
            self.capacity_rejection_total.fetch_add(1, Ordering::Relaxed);
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                format!(
                    "shared memory index document exceeds capacity: length {}, capacity {}",
                    bytes.len(),
                    document_capacity
                ),
            ));
        }
        let len = u64::try_from(bytes.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "shared memory document length is too large")
        })?;
        segment.write_payload(0, &len.to_le_bytes())?;
        segment.write_payload(PAYLOAD_LEN_BYTES, &bytes)?;
        let mut header = segment.header();
        header.generation = document.generation;
        header.store_epoch = document.store_epoch;
        header.operation_seq = document.last_change_seq;
        header.entry_count = document.entries.len() as u64;
        header.current_size_bytes = document_current_size_bytes(document)?;
        segment.write_header(header);
        Ok(())
    }
}

impl SharedIndexBackend for MemorySharedIndexStore {
    fn path(&self) -> &Path {
        &self.path
    }

    fn supports_shared_fill_locks(&self) -> bool {
        true
    }

    fn load(&self) -> io::Result<LoadedSharedIndex> {
        let loaded =
            self.with_document_lock(|_segment, document| loaded_index_from_document(&document))?;
        self.full_reload_total.fetch_add(1, Ordering::Relaxed);
        Ok(loaded)
    }

    fn sync_state(&self) -> io::Result<SharedIndexSyncState> {
        self.with_document_lock(|_segment, document| {
            Ok(SharedIndexSyncState {
                generation: document.generation,
                store_epoch: document.store_epoch,
            })
        })
    }

    fn load_changes_since(&self, after_seq: u64) -> io::Result<LoadedSharedIndexChanges> {
        self.with_document_lock(|_segment, document| {
            let operations = operations_since(&document, after_seq)?;
            Ok(LoadedSharedIndexChanges {
                generation: document.generation,
                store_epoch: document.store_epoch,
                last_change_seq: document.last_change_seq,
                operations,
            })
        })
    }

    fn recreate(
        &self,
        index: &CacheIndex,
        generation: u64,
    ) -> io::Result<AppliedSharedIndexOperations> {
        let _lock = self.lock()?;
        let segment = self.open_or_create_segment()?;
        let previous_rebuild_total =
            self.read_document(&segment).map(|document| document.rebuild_total).unwrap_or(0);
        let mut document = SharedMemoryIndexDocument::empty(segment.header().store_epoch);
        document.generation = generation;
        document.rebuild_total = previous_rebuild_total.saturating_add(1);
        document.entries = entries_from_index(index)?;
        document.admission_counts =
            index.admission_counts.iter().map(|(key, uses)| (key.clone(), *uses)).collect();
        document.invalidations = index
            .invalidations
            .iter()
            .map(serialize_invalidation_rule)
            .collect::<io::Result<Vec<_>>>()?;
        let applied = AppliedSharedIndexOperations {
            generation: document.generation,
            store_epoch: document.store_epoch,
            last_change_seq: document.last_change_seq,
        };
        self.write_document(&segment, &document)?;
        Ok(applied)
    }

    fn apply_operations(
        &self,
        operations: &[SharedIndexOperation],
    ) -> io::Result<AppliedSharedIndexOperations> {
        self.with_document_lock(|segment, mut document| {
            if !operations.is_empty() {
                document.generation = document.generation.saturating_add(1);
                for operation in operations {
                    apply_operation_to_document(&mut document, operation)?;
                    document.last_change_seq = document.last_change_seq.saturating_add(1);
                    document.changes.push(change_record_from_operation(
                        document.last_change_seq,
                        document.generation,
                        operation,
                    )?);
                }
                trim_change_ring(&mut document, self.operation_ring_capacity);
                self.write_document(segment, &document)?;
            }
            Ok(AppliedSharedIndexOperations {
                generation: document.generation,
                store_epoch: document.store_epoch,
                last_change_seq: document.last_change_seq,
            })
        })
    }

    fn try_acquire_fill_lock(
        &self,
        key: &str,
        now_unix_ms: u64,
        lock_age_ms: u64,
        nonce: &str,
        state_json: &[u8],
    ) -> io::Result<SharedFillLockAcquire> {
        self.with_document_lock(|segment, mut document| {
            if let Some(record) = document.fill_locks.get(key)
                && !record.released
                && now_unix_ms.saturating_sub(record.updated_at_unix_ms) <= lock_age_ms
            {
                return Ok(SharedFillLockAcquire::Busy);
            }
            if document.fill_locks.get(key).is_some_and(|record| {
                !record.released
                    && now_unix_ms.saturating_sub(record.updated_at_unix_ms) > lock_age_ms
            }) {
                document.stale_fill_lock_cleanup_total =
                    document.stale_fill_lock_cleanup_total.saturating_add(1);
            }
            document.fill_locks.remove(key);
            document.fill_locks.insert(
                key.to_string(),
                SharedMemoryFillLockRecord {
                    key_hash: cache_key_hash(key),
                    owner_pid: process::id(),
                    owner_generation: document.generation,
                    nonce: nonce.to_string(),
                    acquired_at_unix_ms: now_unix_ms,
                    updated_at_unix_ms: now_unix_ms,
                    released: false,
                    state_json: state_json.to_vec(),
                },
            );
            self.write_document(segment, &document)?;
            Ok(SharedFillLockAcquire::Acquired)
        })
    }

    fn load_fill_lock(&self, key: &str) -> io::Result<Option<SharedFillLockSnapshot>> {
        self.with_document_lock(|_segment, document| {
            Ok(document.fill_locks.get(key).map(|record| SharedFillLockSnapshot {
                nonce: record.nonce.clone(),
                state_json: record.state_json.clone(),
            }))
        })
    }

    fn update_fill_lock(
        &self,
        key: &str,
        nonce: &str,
        updated_at_unix_ms: u64,
        state_json: &[u8],
    ) -> io::Result<()> {
        self.with_document_lock(|segment, mut document| {
            let Some(record) = document.fill_locks.get_mut(key) else {
                return Err(io::Error::new(io::ErrorKind::NotFound, "shared fill lock not found"));
            };
            if record.nonce != nonce {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "shared fill lock nonce mismatch",
                ));
            }
            record.updated_at_unix_ms = updated_at_unix_ms;
            record.state_json = state_json.to_vec();
            self.write_document(segment, &document)
        })
    }

    fn release_fill_lock(&self, key: &str, nonce: &str) -> io::Result<()> {
        self.with_document_lock(|segment, mut document| {
            let Some(record) = document.fill_locks.get_mut(key) else {
                return Ok(());
            };
            if record.nonce != nonce {
                return Ok(());
            }
            record.released = true;
            self.write_document(segment, &document)
        })
    }

    fn fill_lock_status(
        &self,
        key: &str,
        now_unix_ms: u64,
        lock_age_ms: u64,
    ) -> io::Result<SharedFillLockStatus> {
        self.with_document_lock(|_segment, document| {
            Ok(match document.fill_locks.get(key) {
                None => SharedFillLockStatus::Missing,
                Some(record) if record.released => SharedFillLockStatus::Missing,
                Some(record)
                    if now_unix_ms.saturating_sub(record.updated_at_unix_ms) > lock_age_ms =>
                {
                    SharedFillLockStatus::Stale
                }
                Some(_) => SharedFillLockStatus::Fresh,
            })
        })
    }

    fn clear_stale_fill_lock(
        &self,
        key: &str,
        now_unix_ms: u64,
        lock_age_ms: u64,
    ) -> io::Result<bool> {
        self.with_document_lock(|segment, mut document| {
            let Some(record) = document.fill_locks.get(key) else {
                return Ok(false);
            };
            if record.released {
                document.fill_locks.remove(key);
                self.write_document(segment, &document)?;
                return Ok(false);
            }
            if now_unix_ms.saturating_sub(record.updated_at_unix_ms) <= lock_age_ms {
                return Ok(false);
            }
            document.fill_locks.remove(key);
            document.stale_fill_lock_cleanup_total =
                document.stale_fill_lock_cleanup_total.saturating_add(1);
            self.write_document(segment, &document)?;
            Ok(true)
        })
    }

    fn metrics(&self) -> io::Result<SharedIndexMetrics> {
        self.with_document_lock(|segment, document| {
            let bytes = serde_json::to_vec(&document).map_err(invalid_data_error)?;
            let header = segment.header();
            let used_payload_bytes = bytes.len().saturating_add(PAYLOAD_LEN_BYTES);
            let used_bytes = u64::try_from(used_payload_bytes)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "document is too large"))?
                .saturating_add(header.header_len as u64);
            Ok(SharedIndexMetrics {
                shm_capacity_bytes: header.capacity_bytes,
                shm_used_bytes: used_bytes,
                entry_count: header.entry_count,
                current_size_bytes: header.current_size_bytes,
                operation_ring_capacity: header.operation_ring_capacity,
                operation_ring_used: document.changes.len() as u64,
                lock_contention_total: self.lock_contention_total.load(Ordering::Relaxed),
                full_reload_total: self.full_reload_total.load(Ordering::Relaxed),
                rebuild_total: document.rebuild_total,
                stale_fill_lock_cleanup_total: document.stale_fill_lock_cleanup_total,
                capacity_rejection_total: self.capacity_rejection_total.load(Ordering::Relaxed),
            })
        })
    }
}

impl SharedMemoryIndexDocument {
    fn empty(store_epoch: u64) -> Self {
        Self {
            version: 1,
            generation: 0,
            store_epoch,
            last_change_seq: 0,
            rebuild_total: 0,
            stale_fill_lock_cleanup_total: 0,
            entries: BTreeMap::new(),
            admission_counts: BTreeMap::new(),
            invalidations: Vec::new(),
            fill_locks: BTreeMap::new(),
            changes: Vec::new(),
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn loaded_index_from_document(
    document: &SharedMemoryIndexDocument,
) -> io::Result<LoadedSharedIndex> {
    let mut index = CacheIndex::default();
    for (key, entry_json) in &document.entries {
        let entry = deserialize_entry_record(entry_json)?;
        if let Some(existing) = index.insert_entry(key.clone(), entry.clone()) {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(existing.body_size_bytes);
            remove_variant_key(&mut index, &existing.base_key, key);
        }
        index.current_size_bytes = index.current_size_bytes.saturating_add(entry.body_size_bytes);
        add_variant_key(&mut index, entry.base_key.clone(), key.clone());
    }
    index.admission_counts =
        document.admission_counts.iter().map(|(key, uses)| (key.clone(), *uses)).collect();
    index.invalidations = document
        .invalidations
        .iter()
        .map(|rule_json| deserialize_invalidation_rule(rule_json))
        .collect::<io::Result<Vec<_>>>()?;
    Ok(LoadedSharedIndex {
        index,
        generation: document.generation,
        store_epoch: document.store_epoch,
        last_change_seq: document.last_change_seq,
    })
}

fn entries_from_index(index: &CacheIndex) -> io::Result<BTreeMap<String, Vec<u8>>> {
    index
        .entries
        .iter()
        .map(|(key, entry)| Ok((key.clone(), serialize_entry_record(entry)?)))
        .collect()
}

fn apply_operation_to_document(
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

fn change_record_from_operation(
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

fn operations_since(
    document: &SharedMemoryIndexDocument,
    after_seq: u64,
) -> io::Result<Vec<SharedIndexOperation>> {
    let Some(first_change) = document.changes.first() else {
        return Ok(Vec::new());
    };
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

fn trim_change_ring(document: &mut SharedMemoryIndexDocument, operation_ring_capacity: usize) {
    if operation_ring_capacity == 0 {
        document.changes.clear();
        return;
    }
    if document.changes.len() > operation_ring_capacity {
        let drop_count = document.changes.len() - operation_ring_capacity;
        document.changes.drain(0..drop_count);
    }
}

fn document_current_size_bytes(document: &SharedMemoryIndexDocument) -> io::Result<u64> {
    let mut bytes = 0u64;
    for entry_json in document.entries.values() {
        let entry = deserialize_entry_record(entry_json)?;
        bytes = bytes.saturating_add(entry.body_size_bytes as u64);
    }
    Ok(bytes)
}

fn add_variant_key(index: &mut CacheIndex, base_key: String, key: String) {
    let keys = index.variants.entry(base_key).or_default();
    if !keys.contains(&key) {
        keys.push(key);
    }
}

fn remove_variant_key(index: &mut CacheIndex, base_key: &str, key: &str) {
    let Some(keys) = index.variants.get_mut(base_key) else {
        return;
    };
    keys.retain(|candidate| candidate != key);
    if keys.is_empty() {
        index.variants.remove(base_key);
    }
}

fn memory_capacity_bytes() -> usize {
    std::env::var("RGINX_CACHE_SHARED_INDEX_MEMORY_BYTES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_SHM_CAPACITY_BYTES)
}

fn memory_operation_ring_capacity() -> usize {
    std::env::var("RGINX_CACHE_SHARED_INDEX_MEMORY_CHANGES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(DEFAULT_OPERATION_RING_CAPACITY as usize)
}

#[cfg(all(test, target_os = "linux"))]
pub(super) fn unlink_for_zone(zone: &rginx_core::CacheZone) -> io::Result<()> {
    let identity = format!("{}:{}", zone.name, zone.path.display());
    let segment_config =
        SharedMemorySegmentConfig::for_identity(&identity, memory_capacity_bytes());
    SharedMemorySegment::unlink(&segment_config)
}

#[cfg(all(test, target_os = "linux"))]
pub(super) fn corrupt_header_for_zone(zone: &rginx_core::CacheZone) -> io::Result<()> {
    let store = MemorySharedIndexStore::new(zone);
    let _lock = store.lock()?;
    let segment = store.open_or_create_segment()?;
    let mut header = segment.header();
    header.abi_version = header.abi_version.saturating_add(1);
    segment.write_header(header);
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
pub(super) fn corrupt_document_for_zone(zone: &rginx_core::CacheZone) -> io::Result<()> {
    let store = MemorySharedIndexStore::new(zone);
    let _lock = store.lock()?;
    let segment = store.open_or_create_segment()?;
    let invalid_len =
        u64::try_from(segment.payload_capacity()).unwrap_or(u64::MAX).saturating_add(1);
    segment.write_payload(0, &invalid_len.to_le_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::cache::{
        CacheIndexEntry, CacheIndexEntryKind, CacheInvalidationRule, CacheInvalidationSelector,
    };

    #[test]
    fn memory_backend_shares_snapshot_between_store_instances() {
        let temp = tempfile::tempdir().expect("cache temp dir should exist");
        let zone = test_zone(temp.path());
        let store_a = MemorySharedIndexStore::new(&zone);
        let store_b = MemorySharedIndexStore::new(&zone);
        let _ = SharedMemorySegment::unlink(&store_a.segment_config);
        let mut index = CacheIndex::default();
        index.insert_entry("https:example.com:/shm".to_string(), test_entry("/shm"));
        index.admission_counts.insert("https:example.com:/shm".to_string(), 3);

        let applied = store_a.recreate(&index, 11).expect("shm recreate should succeed");
        assert_eq!(applied.generation, 11);
        assert_eq!(applied.last_change_seq, 0);

        let loaded = store_b.load().expect("second shm store should load snapshot");
        assert_eq!(loaded.generation, 11);
        assert_eq!(loaded.index.admission_counts.get("https:example.com:/shm"), Some(&3));
        assert!(loaded.index.entries.contains_key("https:example.com:/shm"));
        let _ = SharedMemorySegment::unlink(&store_a.segment_config);
    }

    #[test]
    fn memory_backend_replays_bounded_changes_and_falls_back_when_gap_exists() {
        let temp = tempfile::tempdir().expect("cache temp dir should exist");
        let zone = test_zone(temp.path());
        let mut store = MemorySharedIndexStore::new(&zone);
        store.operation_ring_capacity = 1;
        store.segment_config = store.segment_config.clone().with_operation_ring_capacity(1);
        let _ = SharedMemorySegment::unlink(&store.segment_config);

        store.recreate(&CacheIndex::default(), 1).expect("shm recreate should succeed");
        store
            .apply_operations(&[SharedIndexOperation::UpsertEntry {
                key: "https:example.com:/one".to_string(),
                entry: test_entry("/one"),
            }])
            .expect("first shm operation should apply");
        store
            .apply_operations(&[SharedIndexOperation::SetAdmissionCount {
                key: "https:example.com:/one".to_string(),
                uses: 2,
            }])
            .expect("second shm operation should apply");

        let replay = store.load_changes_since(1).expect("latest change should replay");
        assert_eq!(replay.operations.len(), 1);
        assert!(matches!(replay.operations[0], SharedIndexOperation::SetAdmissionCount { .. }));

        let gap = store.load_changes_since(0).expect("gap should request full reload");
        assert!(gap.operations.is_empty());
        assert_eq!(gap.last_change_seq, 2);

        let loaded = store.load().expect("full reload should remain available");
        assert!(loaded.index.entries.contains_key("https:example.com:/one"));
        assert_eq!(loaded.index.admission_counts.get("https:example.com:/one"), Some(&2));
        let _ = SharedMemorySegment::unlink(&store.segment_config);
    }

    #[test]
    fn memory_backend_persists_invalidations_and_replays_delta() {
        let temp = tempfile::tempdir().expect("cache temp dir should exist");
        let zone = test_zone(temp.path());
        let store = MemorySharedIndexStore::new(&zone);
        let _ = SharedMemorySegment::unlink(&store.segment_config);

        store.recreate(&CacheIndex::default(), 1).expect("shm recreate should succeed");
        let rule = CacheInvalidationRule {
            selector: CacheInvalidationSelector::Exact("https:example.com:/invalidate".to_string()),
            created_at_unix_ms: 1_000,
        };
        store
            .apply_operations(&[SharedIndexOperation::AddInvalidation { rule: rule.clone() }])
            .expect("invalidation should apply");

        let loaded = store.load().expect("invalidations should load from shm");
        assert_eq!(loaded.index.invalidations, vec![rule.clone()]);

        let replay = store.load_changes_since(0).expect("invalidation delta should replay");
        assert_eq!(replay.operations.len(), 1);
        assert!(matches!(replay.operations[0], SharedIndexOperation::AddInvalidation { .. }));
        let _ = SharedMemorySegment::unlink(&store.segment_config);
    }

    #[test]
    fn memory_backend_metrics_track_reloads_contention_and_ring_usage() {
        let temp = tempfile::tempdir().expect("cache temp dir should exist");
        let zone = test_zone(temp.path());
        let mut store = Arc::new(MemorySharedIndexStore::new(&zone));
        {
            let store_mut = Arc::get_mut(&mut store).expect("store should be uniquely owned");
            store_mut.operation_ring_capacity = 1;
            store_mut.segment_config =
                store_mut.segment_config.clone().with_operation_ring_capacity(1);
        }
        let _ = SharedMemorySegment::unlink(&store.segment_config);

        store.recreate(&CacheIndex::default(), 1).expect("shm recreate should succeed");
        store
            .apply_operations(&[SharedIndexOperation::UpsertEntry {
                key: "https:example.com:/metrics".to_string(),
                entry: test_entry("/metrics"),
            }])
            .expect("upsert should succeed");
        let loaded = store.load().expect("full reload should succeed");
        assert!(loaded.index.entries.contains_key("https:example.com:/metrics"));

        let holder = {
            let store = store.clone();
            thread::spawn(move || {
                store
                    .with_document_lock(|_, _| {
                        thread::sleep(Duration::from_millis(50));
                        Ok(())
                    })
                    .expect("lock holder should succeed");
            })
        };
        thread::sleep(Duration::from_millis(10));
        let metrics = store.metrics().expect("metrics should load");
        holder.join().expect("lock holder should join");

        assert_eq!(metrics.rebuild_total, 1);
        assert_eq!(metrics.full_reload_total, 1);
        assert_eq!(metrics.operation_ring_capacity, 1);
        assert_eq!(metrics.operation_ring_used, 1);
        assert!(metrics.shm_used_bytes > 0);
        assert!(metrics.lock_contention_total >= 1);
        let _ = SharedMemorySegment::unlink(&store.segment_config);
    }

    #[test]
    fn memory_backend_counts_capacity_rejections_for_oversized_documents() {
        let temp = tempfile::tempdir().expect("cache temp dir should exist");
        let zone = test_zone(temp.path());
        let mut store = MemorySharedIndexStore::new(&zone);
        store.segment_config.capacity_bytes = 1_024;
        store.segment_config = store.segment_config.clone().with_operation_ring_capacity(1);
        let _ = SharedMemorySegment::unlink(&store.segment_config);

        store.recreate(&CacheIndex::default(), 1).expect("empty shm recreate should succeed");
        let oversized_path = format!("/{}", "x".repeat(4_096));
        let result = store.apply_operations(&[SharedIndexOperation::UpsertEntry {
            key: format!("https:example.com:{oversized_path}"),
            entry: test_entry(&oversized_path),
        }]);
        let error = match result {
            Ok(_) => panic!("oversized document should be rejected"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::OutOfMemory);

        let metrics = store.metrics().expect("metrics should load");
        assert_eq!(metrics.capacity_rejection_total, 1);
        assert_eq!(metrics.rebuild_total, 1);
        let _ = SharedMemorySegment::unlink(&store.segment_config);
    }

    fn test_zone(path: &Path) -> rginx_core::CacheZone {
        rginx_core::CacheZone {
            name: format!("shm-test-{}", unique_suffix()),
            path: path.to_path_buf(),
            max_size_bytes: Some(1024 * 1024),
            inactive: std::time::Duration::from_secs(60),
            default_ttl: std::time::Duration::from_secs(60),
            max_entry_bytes: 1024,
            path_levels: vec![2],
            loader_batch_entries: 100,
            loader_sleep: std::time::Duration::ZERO,
            manager_batch_entries: 100,
            manager_sleep: std::time::Duration::ZERO,
            inactive_cleanup_interval: std::time::Duration::from_secs(60),
            shared_index: true,
        }
    }

    fn test_entry(path: &str) -> CacheIndexEntry {
        CacheIndexEntry {
            kind: CacheIndexEntryKind::Response,
            hash: format!("hash-{path}"),
            base_key: format!("https:example.com:{path}"),
            stored_at_unix_ms: 1_000,
            vary: Vec::new(),
            tags: Vec::new(),
            body_size_bytes: 3,
            expires_at_unix_ms: 60_000,
            grace_until_unix_ms: None,
            keep_until_unix_ms: None,
            stale_if_error_until_unix_ms: None,
            stale_while_revalidate_until_unix_ms: None,
            requires_revalidation: false,
            must_revalidate: false,
            last_access_unix_ms: 1_000,
        }
    }

    fn unique_suffix() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    }
}
