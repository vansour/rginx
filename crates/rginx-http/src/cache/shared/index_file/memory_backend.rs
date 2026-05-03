use super::codec::serialize_invalidation_rule;
use super::{
    AppliedSharedIndexOperations, LoadedSharedIndex, LoadedSharedIndexChanges,
    SharedFillLockAcquire, SharedFillLockSnapshot, SharedFillLockStatus, SharedIndexBackend,
    SharedIndexMetrics, SharedIndexOperation, SharedIndexSyncState, invalid_data_error,
};
use crate::cache::CacheIndex;
use crate::cache::entry::cache_key_hash;
use crate::cache::shared::memory::{SharedMemorySegment, SharedMemorySegmentConfig};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

mod changes;
mod document;
#[cfg(test)]
mod tests;

use changes::{
    apply_operation_to_document, change_record_from_operation, operations_since, trim_change_ring,
};
use document::{
    PAYLOAD_LEN_BYTES, SharedMemoryFillLockRecord, SharedMemoryIndexDocument,
    document_current_size_bytes, entries_from_index, loaded_index_from_document,
};

const DEFAULT_SHM_CAPACITY_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_OPERATION_RING_CAPACITY: u64 = 4_096;

pub(super) struct MemorySharedIndexStore {
    path: PathBuf,
    lock_path: PathBuf,
    segment_config: SharedMemorySegmentConfig,
    operation_ring_capacity: usize,
    lock_contention_total: AtomicU64,
    full_reload_total: AtomicU64,
    capacity_rejection_total: AtomicU64,
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
            match document.fill_locks.get(key) {
                Some(record) if !record.released => {
                    let age = now_unix_ms.saturating_sub(record.updated_at_unix_ms);
                    if age <= lock_age_ms {
                        return Ok(SharedFillLockAcquire::Busy);
                    }
                    document.stale_fill_lock_cleanup_total =
                        document.stale_fill_lock_cleanup_total.saturating_add(1);
                }
                _ => {}
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

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
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
    tests::unlink_for_zone(zone)
}

#[cfg(all(test, target_os = "linux"))]
pub(super) fn corrupt_header_for_zone(zone: &rginx_core::CacheZone) -> io::Result<()> {
    tests::corrupt_header_for_zone(zone)
}

#[cfg(all(test, target_os = "linux"))]
pub(super) fn corrupt_document_for_zone(zone: &rginx_core::CacheZone) -> io::Result<()> {
    tests::corrupt_document_for_zone(zone)
}
