use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::super::super::{CacheIndex, CacheIndexEntry, CachedVaryHeaderValue};
use super::{LoadedSharedIndex, SHARED_INDEX_SCHEMA_VERSION, invalid_data_error};

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
    requires_revalidation: Option<bool>,
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
    requires_revalidation: Option<bool>,
    #[serde(default)]
    must_revalidate: bool,
    last_access_unix_ms: u64,
}

#[derive(Debug, Deserialize)]
struct LegacySharedAdmissionCount {
    key: String,
    uses: u64,
}

pub(super) fn load_legacy_shared_index_bytes(
    bytes: &[u8],
    path: &Path,
) -> io::Result<LoadedSharedIndex> {
    let file: LegacySharedIndexFile = serde_json::from_slice(bytes).map_err(invalid_data_error)?;
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

    Ok(index_from_legacy_shared_file(file))
}

pub(super) fn serialize_entry_record(entry: &CacheIndexEntry) -> io::Result<Vec<u8>> {
    serde_json::to_vec(&SharedIndexEntryRecord {
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
        requires_revalidation: Some(entry.requires_revalidation),
        must_revalidate: entry.must_revalidate,
        last_access_unix_ms: entry.last_access_unix_ms,
    })
    .map_err(invalid_data_error)
}

pub(super) fn deserialize_entry_record(entry_json: &[u8]) -> io::Result<CacheIndexEntry> {
    let record: SharedIndexEntryRecord =
        serde_json::from_slice(entry_json).map_err(invalid_data_error)?;
    cache_index_entry_from_record(record)
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
        requires_revalidation: record.requires_revalidation.unwrap_or(record.must_revalidate),
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
            requires_revalidation: entry.requires_revalidation.unwrap_or(entry.must_revalidate),
            must_revalidate: entry.must_revalidate,
            last_access_unix_ms: entry.last_access_unix_ms,
        };
        index.current_size_bytes =
            index.current_size_bytes.saturating_add(index_entry.body_size_bytes);
        index.variants.entry(entry.base_key).or_default().push(key.clone());
        index.insert_entry(key, index_entry);
    }
    for admission in file.admission_counts {
        index.admission_counts.insert(admission.key, admission.uses);
    }
    LoadedSharedIndex { index, generation: file.generation }
}
