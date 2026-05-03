use std::collections::BTreeMap;
use std::io;

use serde::{Deserialize, Serialize};

use super::super::LoadedSharedIndex;
use super::super::codec::{
    deserialize_entry_record, deserialize_invalidation_rule, serialize_entry_record,
};
use crate::cache::CacheIndex;

pub(super) const PAYLOAD_LEN_BYTES: usize = 8;

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct SharedMemoryIndexDocument {
    pub(super) version: u32,
    pub(super) generation: u64,
    pub(super) store_epoch: u64,
    pub(super) last_change_seq: u64,
    #[serde(default)]
    pub(super) rebuild_total: u64,
    #[serde(default)]
    pub(super) stale_fill_lock_cleanup_total: u64,
    pub(super) entries: BTreeMap<String, Vec<u8>>,
    pub(super) admission_counts: BTreeMap<String, u64>,
    pub(super) invalidations: Vec<Vec<u8>>,
    #[serde(default)]
    pub(super) fill_locks: BTreeMap<String, SharedMemoryFillLockRecord>,
    pub(super) changes: Vec<SharedMemoryChangeRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SharedMemoryChangeRecord {
    pub(super) seq: u64,
    pub(super) generation: u64,
    pub(super) op_kind: u8,
    pub(super) key: String,
    pub(super) entry_json: Option<Vec<u8>>,
    pub(super) uses: Option<u64>,
    #[serde(default)]
    pub(super) last_access_unix_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct SharedMemoryFillLockRecord {
    pub(super) key_hash: String,
    pub(super) owner_pid: u32,
    pub(super) owner_generation: u64,
    pub(super) nonce: String,
    pub(super) acquired_at_unix_ms: u64,
    pub(super) updated_at_unix_ms: u64,
    #[serde(default)]
    pub(super) released: bool,
    pub(super) state_json: Vec<u8>,
}

impl SharedMemoryIndexDocument {
    pub(super) fn empty(store_epoch: u64) -> Self {
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

pub(super) fn loaded_index_from_document(
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

pub(super) fn entries_from_index(index: &CacheIndex) -> io::Result<BTreeMap<String, Vec<u8>>> {
    index
        .entries
        .iter()
        .map(|(key, entry)| Ok((key.clone(), serialize_entry_record(entry)?)))
        .collect()
}

pub(super) fn document_current_size_bytes(document: &SharedMemoryIndexDocument) -> io::Result<u64> {
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
