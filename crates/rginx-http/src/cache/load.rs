use std::fs;
use std::io;
use std::path::Path;
use std::thread;

use rginx_core::CacheZone;
use serde::Deserialize;

use super::entry::{cache_key_hash, cache_paths_for_zone};
use super::store::eviction_candidates;
use super::{CacheIndex, CacheIndexEntry, CachedVaryHeaderValue};

#[derive(Debug, Deserialize)]
struct ScannedCacheMetadata {
    #[serde(default)]
    key: String,
    #[serde(default)]
    base_key: String,
    #[serde(default)]
    vary: Vec<ScannedVaryHeader>,
    #[serde(default)]
    stored_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    #[serde(default)]
    stale_if_error_until_unix_ms: Option<u64>,
    #[serde(default)]
    stale_while_revalidate_until_unix_ms: Option<u64>,
    #[serde(default)]
    requires_revalidation: Option<bool>,
    #[serde(default)]
    must_revalidate: bool,
    body_size_bytes: usize,
}

#[derive(Debug, Deserialize)]
struct ScannedVaryHeader {
    name: String,
    #[serde(default)]
    value: Option<String>,
}

pub(super) fn load_index_from_disk(zone: &CacheZone) -> io::Result<CacheIndex> {
    let mut index = CacheIndex::default();
    if !zone.path.exists() {
        return Ok(index);
    }

    let mut loader = LoaderState::default();
    scan_cache_dir(zone, zone.path.as_path(), &mut index, &mut loader)?;

    for (key, entry) in eviction_candidates(&mut index, zone.max_size_bytes) {
        remove_variant_key(&mut index.variants, &entry.base_key, &key);
        remove_cache_files(zone, &entry.hash);
    }

    Ok(index)
}

fn scan_cache_dir(
    zone: &CacheZone,
    dir: &Path,
    index: &mut CacheIndex,
    loader: &mut LoaderState,
) -> io::Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(path = %dir.display(), %error, "failed to read cache directory; skipping");
            return Ok(());
        }
    };
    for entry in entries {
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
        loader.maybe_sleep(zone);
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            tracing::warn!(path = %path.display(), "failed to read cache directory entry type; skipping");
            continue;
        };
        if file_type.is_dir() {
            if let Err(error) = scan_cache_dir(zone, path.as_path(), index, loader) {
                tracing::warn!(
                    path = %path.display(),
                    %error,
                    "failed to scan cache subdirectory; skipping"
                );
            }
            continue;
        }
        let Some(hash) = metadata_hash(&path) else {
            if let Some(hash) = body_hash(&path)
                && !cache_paths_for_zone(zone, &hash).metadata.exists()
            {
                remove_cache_files(zone, &hash);
            }
            continue;
        };
        let Some((key, index_entry)) = load_cache_index_entry(zone, &hash) else {
            continue;
        };

        if let Some(existing) = index.insert_entry(key.clone(), index_entry.clone()) {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(existing.body_size_bytes);
            remove_variant_key(&mut index.variants, &existing.base_key, &key);
        }
        index.current_size_bytes =
            index.current_size_bytes.saturating_add(index_entry.body_size_bytes);
        add_variant_key(&mut index.variants, index_entry.base_key.clone(), key);
    }
    Ok(())
}

fn load_cache_index_entry(zone: &CacheZone, hash: &str) -> Option<(String, CacheIndexEntry)> {
    let paths = cache_paths_for_zone(zone, hash);
    let Ok(metadata) = read_cache_metadata(&paths.metadata) else {
        remove_cache_files(zone, hash);
        return None;
    };
    if metadata.key.is_empty() || cache_key_hash(&metadata.key) != hash {
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

    let ScannedCacheMetadata {
        key,
        base_key,
        vary,
        stored_at_unix_ms,
        expires_at_unix_ms,
        stale_if_error_until_unix_ms,
        stale_while_revalidate_until_unix_ms,
        requires_revalidation,
        must_revalidate,
        body_size_bytes,
    } = metadata;
    let Some(vary) = parse_vary_headers(vary) else {
        remove_cache_files(zone, hash);
        return None;
    };

    Some((
        key.clone(),
        CacheIndexEntry {
            hash: hash.to_string(),
            base_key: if base_key.is_empty() { key } else { base_key },
            vary,
            body_size_bytes,
            expires_at_unix_ms,
            stale_if_error_until_unix_ms,
            stale_while_revalidate_until_unix_ms,
            requires_revalidation: requires_revalidation.unwrap_or(must_revalidate),
            must_revalidate,
            last_access_unix_ms: stored_at_unix_ms,
        },
    ))
}

fn parse_vary_headers(vary: Vec<ScannedVaryHeader>) -> Option<Vec<CachedVaryHeaderValue>> {
    let mut parsed = Vec::with_capacity(vary.len());
    for header in vary {
        let name = header.name.parse::<http::header::HeaderName>().ok()?;
        parsed.push(CachedVaryHeaderValue { name, value: header.value });
    }
    Some(parsed)
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
    let paths = cache_paths_for_zone(zone, hash);
    let _ = fs::remove_file(paths.metadata);
    let _ = fs::remove_file(paths.body);
}

#[derive(Default)]
struct LoaderState {
    processed_entries: usize,
}

impl LoaderState {
    fn maybe_sleep(&mut self, zone: &CacheZone) {
        self.processed_entries = self.processed_entries.saturating_add(1);
        if zone.loader_batch_entries == 0
            || zone.loader_sleep.is_zero()
            || !self.processed_entries.is_multiple_of(zone.loader_batch_entries)
        {
            return;
        }
        if let Ok(handle) = tokio::runtime::Handle::try_current()
            && handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread
        {
            tokio::task::block_in_place(|| thread::sleep(zone.loader_sleep));
            return;
        }
        thread::sleep(zone.loader_sleep);
    }
}

fn add_variant_key(
    variants: &mut std::collections::HashMap<String, Vec<String>>,
    base_key: String,
    key: String,
) {
    let entry = variants.entry(base_key).or_default();
    if !entry.contains(&key) {
        entry.push(key);
    }
}

fn remove_variant_key(
    variants: &mut std::collections::HashMap<String, Vec<String>>,
    base_key: &str,
    key: &str,
) {
    let Some(keys) = variants.get_mut(base_key) else {
        return;
    };
    keys.retain(|candidate| candidate != key);
    if keys.is_empty() {
        variants.remove(base_key);
    }
}
