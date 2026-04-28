use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::entry::cache_key_hash;
use super::load::load_index_from_disk;
use super::store::lock_index;
use super::{CacheIndex, CacheIndexEntry, CacheZoneRuntime, CachedVaryHeaderValue, unix_time_ms};

const SHARED_INDEX_FILE_VERSION: u8 = 1;
const SHARED_INDEX_FILENAME: &str = ".rginx-index.json";
const SHARED_FILL_LOCK_PREFIX: &str = ".rginx-fill-";
const SHARED_FILL_LOCK_SUFFIX: &str = ".lock";

pub(super) struct LoadedSharedIndex {
    pub(super) index: CacheIndex,
    pub(super) generation: u64,
    pub(super) modified_unix_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct SharedIndexFile {
    version: u8,
    generation: u64,
    entries: Vec<SharedIndexEntry>,
    #[serde(default)]
    admission_counts: Vec<SharedAdmissionCount>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SharedIndexEntry {
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

#[derive(Debug, Serialize, Deserialize)]
struct SharedVaryHeader {
    name: String,
    #[serde(default)]
    value: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SharedAdmissionCount {
    key: String,
    uses: u64,
}

pub(super) fn shared_fill_lock_path(zone: &rginx_core::CacheZone, key: &str) -> PathBuf {
    zone.path
        .join(format!("{SHARED_FILL_LOCK_PREFIX}{}{SHARED_FILL_LOCK_SUFFIX}", cache_key_hash(key)))
}

pub(super) fn load_shared_index_from_disk(
    zone: &rginx_core::CacheZone,
) -> io::Result<Option<LoadedSharedIndex>> {
    let path = shared_index_path(zone);
    if !path.exists() {
        return Ok(None);
    }

    let bytes = fs::read(&path)?;
    let file: SharedIndexFile = serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if file.version != SHARED_INDEX_FILE_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unsupported shared index file version `{}` in `{}`",
                file.version,
                path.display()
            ),
        ));
    }

    let modified_unix_ms = file_modified_unix_ms(&path)?.unwrap_or_default();
    let generation = file.generation;
    Ok(Some(LoadedSharedIndex {
        index: index_from_shared_file(file),
        generation,
        modified_unix_ms,
    }))
}

pub(super) fn bootstrap_shared_index(
    zone: &rginx_core::CacheZone,
) -> io::Result<(CacheIndex, u64, u64)> {
    if !zone.shared_index {
        return Ok((load_index_from_disk(zone)?, 0, 0));
    }

    match load_shared_index_from_disk(zone) {
        Ok(Some(loaded)) => Ok((loaded.index, loaded.generation, loaded.modified_unix_ms)),
        Ok(None) => {
            let index = load_index_from_disk(zone)?;
            let modified_unix_ms = persist_shared_index_to_disk(zone, &index, 1, 0)?;
            Ok((index, 1, modified_unix_ms))
        }
        Err(error) => {
            tracing::warn!(
                zone = %zone.name,
                path = %shared_index_path(zone).display(),
                %error,
                "failed to load shared cache index; rebuilding from cache files"
            );
            let index = load_index_from_disk(zone)?;
            let modified_unix_ms = persist_shared_index_to_disk(zone, &index, 1, 0)?;
            Ok((index, 1, modified_unix_ms))
        }
    }
}

pub(super) async fn sync_zone_shared_index_if_needed(zone: &std::sync::Arc<CacheZoneRuntime>) {
    if !zone.config.shared_index {
        return;
    }

    let Some(disk_modified_unix_ms) = shared_index_modified_unix_ms(zone.config.as_ref()) else {
        return;
    };
    if disk_modified_unix_ms
        <= zone.shared_index_last_modified_unix_ms.load(std::sync::atomic::Ordering::Relaxed)
    {
        return;
    }

    let _sync_guard = zone.shared_index_sync_lock.lock().await;
    let Some(disk_modified_unix_ms) = shared_index_modified_unix_ms(zone.config.as_ref()) else {
        return;
    };
    if disk_modified_unix_ms
        <= zone.shared_index_last_modified_unix_ms.load(std::sync::atomic::Ordering::Relaxed)
    {
        return;
    }

    let loaded = match run_blocking(|| load_shared_index_from_disk(zone.config.as_ref())) {
        Ok(Some(loaded)) => loaded,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(
                zone = %zone.config.name,
                path = %shared_index_path(zone.config.as_ref()).display(),
                %error,
                "failed to reload shared cache index; keeping local index"
            );
            return;
        }
    };

    let mut index = lock_index(&zone.index);
    *index = loaded.index;
    drop(index);
    zone.shared_index_generation.store(loaded.generation, std::sync::atomic::Ordering::Relaxed);
    zone.shared_index_last_modified_unix_ms
        .store(loaded.modified_unix_ms, std::sync::atomic::Ordering::Relaxed);
}

pub(super) async fn persist_zone_shared_index(zone: &std::sync::Arc<CacheZoneRuntime>) {
    if !zone.config.shared_index {
        return;
    }

    let _sync_guard = zone.shared_index_sync_lock.lock().await;
    let next_generation =
        zone.shared_index_generation.load(std::sync::atomic::Ordering::Relaxed) + 1;
    let minimum_modified_unix_ms = zone
        .shared_index_last_modified_unix_ms
        .load(std::sync::atomic::Ordering::Relaxed)
        .saturating_add(1);
    let snapshot = {
        let index = lock_index(&zone.index);
        index.clone()
    };
    match run_blocking(|| {
        persist_shared_index_to_disk(
            zone.config.as_ref(),
            &snapshot,
            next_generation,
            minimum_modified_unix_ms,
        )
    }) {
        Ok(modified_unix_ms) => {
            zone.shared_index_generation
                .store(next_generation, std::sync::atomic::Ordering::Relaxed);
            zone.shared_index_last_modified_unix_ms
                .store(modified_unix_ms, std::sync::atomic::Ordering::Relaxed);
        }
        Err(error) => {
            tracing::warn!(
                zone = %zone.config.name,
                path = %shared_index_path(zone.config.as_ref()).display(),
                %error,
                "failed to persist shared cache index"
            );
        }
    }
}

fn shared_index_path(zone: &rginx_core::CacheZone) -> PathBuf {
    zone.path.join(SHARED_INDEX_FILENAME)
}

fn next_shared_index_modified_unix_ms(
    zone: &rginx_core::CacheZone,
    minimum_modified_unix_ms: u64,
) -> u64 {
    unix_time_ms(SystemTime::now())
        .max(shared_index_modified_unix_ms(zone).unwrap_or_default().saturating_add(1))
        .max(minimum_modified_unix_ms)
}

fn shared_index_modified_unix_ms(zone: &rginx_core::CacheZone) -> Option<u64> {
    file_modified_unix_ms(&shared_index_path(zone)).ok().flatten()
}

fn set_file_modified_unix_ms(path: &std::path::Path, modified_unix_ms: u64) -> io::Result<()> {
    let file = fs::OpenOptions::new().write(true).open(path)?;
    file.set_modified(UNIX_EPOCH + Duration::from_millis(modified_unix_ms))
}

fn persist_shared_index_to_disk(
    zone: &rginx_core::CacheZone,
    index: &CacheIndex,
    generation: u64,
    minimum_modified_unix_ms: u64,
) -> io::Result<u64> {
    let path = shared_index_path(zone);
    let tmp = path.with_extension(format!("json.tmp-{}", unix_time_ms(SystemTime::now())));
    let target_modified_unix_ms =
        next_shared_index_modified_unix_ms(zone, minimum_modified_unix_ms);
    let payload = serde_json::to_vec(&shared_file_from_index(index, generation))
        .map_err(|error| io::Error::other(error.to_string()))?;

    fs::write(&tmp, payload)?;
    fs::rename(&tmp, &path)?;
    // Keep the sidecar mtime strictly monotonic so other runtimes can detect
    // back-to-back shared-index updates even when the filesystem timestamp
    // resolution is coarser than our write cadence.
    set_file_modified_unix_ms(&path, target_modified_unix_ms)?;
    file_modified_unix_ms(&path)?.ok_or_else(|| {
        io::Error::other(format!(
            "missing modified time for shared cache index `{}`",
            path.display()
        ))
    })
}

fn shared_file_from_index(index: &CacheIndex, generation: u64) -> SharedIndexFile {
    let mut entries = index
        .entries
        .iter()
        .map(|(key, entry)| SharedIndexEntry {
            key: key.clone(),
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
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.key.cmp(&right.key));

    let mut admission_counts = index
        .admission_counts
        .iter()
        .map(|(key, uses)| SharedAdmissionCount { key: key.clone(), uses: *uses })
        .collect::<Vec<_>>();
    admission_counts.sort_by(|left, right| left.key.cmp(&right.key));

    SharedIndexFile { version: SHARED_INDEX_FILE_VERSION, generation, entries, admission_counts }
}

fn index_from_shared_file(file: SharedIndexFile) -> CacheIndex {
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
    index
}

fn file_modified_unix_ms(path: &std::path::Path) -> io::Result<Option<u64>> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let modified = metadata.modified()?;
    Ok(Some(unix_time_ms(modified)))
}

fn run_blocking<T>(operation: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    if let Ok(handle) = tokio::runtime::Handle::try_current()
        && handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
    {
        return tokio::task::block_in_place(operation);
    }
    operation()
}
