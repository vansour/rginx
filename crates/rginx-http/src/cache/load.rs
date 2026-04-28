use std::fs;
use std::io;
use std::path::Path;

use rginx_core::CacheZone;
use serde::Deserialize;

use super::entry::{cache_key_hash, cache_paths, unix_time_ms};
use super::store::eviction_candidates;
use super::{CacheIndex, CacheIndexEntry};

#[derive(Debug, Deserialize)]
struct ScannedCacheMetadata {
    #[serde(default)]
    key: String,
    #[serde(default)]
    stored_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    #[serde(default)]
    stale_if_error_until_unix_ms: Option<u64>,
    #[serde(default)]
    stale_while_revalidate_until_unix_ms: Option<u64>,
    #[serde(default)]
    must_revalidate: bool,
    body_size_bytes: usize,
}

pub(super) fn load_index_from_disk(zone: &CacheZone) -> io::Result<CacheIndex> {
    let mut index = CacheIndex::default();
    if !zone.path.exists() {
        return Ok(index);
    }

    let now = unix_time_ms(std::time::SystemTime::now());
    for prefix_dir in fs::read_dir(&zone.path)? {
        let prefix_dir = match prefix_dir {
            Ok(prefix_dir) => prefix_dir,
            Err(error) => {
                tracing::warn!(
                    path = %zone.path.display(),
                    %error,
                    "failed to read cache zone directory entry; skipping"
                );
                continue;
            }
        };
        let Ok(file_type) = prefix_dir.file_type() else {
            tracing::warn!(
                path = %prefix_dir.path().display(),
                "failed to read cache directory entry type; skipping"
            );
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        scan_prefix_dir(zone, prefix_dir.path().as_path(), now, &mut index)?;
    }

    for hash in eviction_candidates(&mut index, zone.max_size_bytes) {
        remove_cache_files(zone, &hash);
    }

    Ok(index)
}

fn scan_prefix_dir(
    zone: &CacheZone,
    dir: &Path,
    now: u64,
    index: &mut CacheIndex,
) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!(
                    path = %dir.display(),
                    %error,
                    "failed to read cache prefix directory entry; skipping"
                );
                continue;
            }
        };
        let path = entry.path();
        let Some(hash) = metadata_hash(&path) else {
            if let Some(hash) = body_hash(&path)
                && !cache_paths(&zone.path, &hash).metadata.exists()
            {
                remove_cache_files(zone, &hash);
            }
            continue;
        };
        let Some((key, index_entry)) = load_cache_index_entry(zone, &hash, now) else {
            continue;
        };

        if let Some(existing) = index.entries.insert(key, index_entry.clone()) {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(existing.body_size_bytes);
        }
        index.current_size_bytes =
            index.current_size_bytes.saturating_add(index_entry.body_size_bytes);
    }
    Ok(())
}

fn load_cache_index_entry(
    zone: &CacheZone,
    hash: &str,
    now: u64,
) -> Option<(String, CacheIndexEntry)> {
    let paths = cache_paths(&zone.path, hash);
    let Ok(metadata) = read_cache_metadata(&paths.metadata) else {
        remove_cache_files(zone, hash);
        return None;
    };
    if metadata.key.is_empty() || cache_key_hash(&metadata.key) != hash {
        remove_cache_files(zone, hash);
        return None;
    }
    let mut stale_windows =
        [metadata.stale_if_error_until_unix_ms, metadata.stale_while_revalidate_until_unix_ms]
            .into_iter()
            .flatten();
    let beyond_stale_windows = stale_windows
        .next()
        .is_some_and(|first| now > first && stale_windows.all(|value| now > value));
    if now > metadata.expires_at_unix_ms && beyond_stale_windows && !metadata.must_revalidate {
        remove_cache_files(zone, hash);
        return None;
    }

    let Ok(body_metadata) = fs::metadata(&paths.body) else {
        remove_cache_files(zone, hash);
        return None;
    };
    let body_size = body_metadata.len();
    let Ok(body_size) = usize::try_from(body_size) else {
        remove_cache_files(zone, hash);
        return None;
    };
    if body_size != metadata.body_size_bytes {
        remove_cache_files(zone, hash);
        return None;
    }

    Some((
        metadata.key,
        CacheIndexEntry {
            hash: hash.to_string(),
            body_size_bytes: metadata.body_size_bytes,
            expires_at_unix_ms: metadata.expires_at_unix_ms,
            stale_if_error_until_unix_ms: metadata.stale_if_error_until_unix_ms,
            stale_while_revalidate_until_unix_ms: metadata.stale_while_revalidate_until_unix_ms,
            must_revalidate: metadata.must_revalidate,
            last_access_unix_ms: metadata.stored_at_unix_ms,
        },
    ))
}

fn read_cache_metadata(path: &Path) -> io::Result<ScannedCacheMetadata> {
    let metadata = fs::read(path)?;
    serde_json::from_slice(&metadata)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn metadata_hash(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let hash = name.strip_suffix(".meta.json")?;
    (hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| hash.to_string())
}

fn body_hash(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let hash = name.strip_suffix(".body")?;
    (hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| hash.to_string())
}

fn remove_cache_files(zone: &CacheZone, hash: &str) {
    let paths = cache_paths(&zone.path, hash);
    let _ = fs::remove_file(paths.metadata);
    let _ = fs::remove_file(paths.body);
}
