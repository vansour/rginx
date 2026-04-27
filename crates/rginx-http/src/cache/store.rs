use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use http::Response;
use http_body_util::BodyExt;
use tokio::fs;

use crate::handler::{HttpResponse, full_body};

use super::entry::{cache_key_hash, cache_metadata, cache_paths, unix_time_ms, write_cache_entry};
use super::policy::{response_is_storable, response_ttl};
use super::{CacheIndex, CacheIndexEntry, CacheStoreContext, CacheZoneRuntime};

pub(super) struct CacheStoreError {
    source: Box<dyn std::error::Error + Send + Sync>,
}

impl std::fmt::Display for CacheStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.source)
    }
}

impl std::fmt::Debug for CacheStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("CacheStoreError").field("source", &self.source).finish()
    }
}

impl std::error::Error for CacheStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub(super) async fn store_response(
    context: CacheStoreContext,
    response: HttpResponse,
) -> std::result::Result<HttpResponse, CacheStoreError> {
    if !response_is_storable(&context, &response) {
        return Ok(response);
    }

    let (parts, body) = response.into_parts();
    let collected = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            return Err(CacheStoreError { source: error });
        }
    };

    if collected.len() > context.zone.config.max_entry_bytes {
        return Ok(Response::from_parts(parts, full_body(collected)));
    }

    let now = unix_time_ms(SystemTime::now());
    let ttl = response_ttl(&parts.headers, context.zone.config.default_ttl);
    if ttl.is_zero() {
        return Ok(Response::from_parts(parts, full_body(collected)));
    }
    let expires_at_unix_ms = now.saturating_add(duration_to_ms(ttl));
    let metadata = cache_metadata(
        context.key.clone(),
        parts.status,
        &parts.headers,
        now,
        expires_at_unix_ms,
        collected.len(),
    );
    let hash = cache_key_hash(&context.key);
    let paths = cache_paths(&context.zone.config.path, &hash);
    let _io_guard = context.zone.io_lock.lock().await;

    if let Err(error) = write_cache_entry(&paths, &metadata, &collected).await {
        tracing::warn!(
            zone = %context.zone.config.name,
            key_hash = %hash,
            %error,
            "failed to write cache entry"
        );
    } else {
        update_index_after_store(
            &context.zone,
            context.key,
            CacheIndexEntry {
                hash,
                body_size_bytes: metadata.body_size_bytes,
                expires_at_unix_ms,
                last_access_unix_ms: now,
            },
        )
        .await;
    }

    Ok(Response::from_parts(parts, full_body(collected)))
}

async fn update_index_after_store(
    zone: &Arc<CacheZoneRuntime>,
    key: String,
    entry: CacheIndexEntry,
) {
    let evictions = {
        let mut index = lock_index(&zone.index);
        if let Some(existing) = index.entries.insert(key, entry.clone()) {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(existing.body_size_bytes);
        }
        index.current_size_bytes = index.current_size_bytes.saturating_add(entry.body_size_bytes);
        eviction_candidates(&mut index, zone.config.max_size_bytes)
    };

    for hash in evictions {
        let paths = cache_paths(&zone.config.path, &hash);
        let _ = fs::remove_file(paths.metadata).await;
        let _ = fs::remove_file(paths.body).await;
    }
}

pub(super) fn eviction_candidates(
    index: &mut CacheIndex,
    max_size_bytes: Option<usize>,
) -> Vec<String> {
    let Some(max_size_bytes) = max_size_bytes else {
        return Vec::new();
    };
    if index.current_size_bytes <= max_size_bytes {
        return Vec::new();
    }

    let mut entries =
        index.entries.iter().map(|(key, entry)| (key.clone(), entry.clone())).collect::<Vec<_>>();
    entries.sort_by_key(|(_, entry)| entry.last_access_unix_ms);

    let mut evicted = Vec::new();
    for (key, entry) in entries {
        if index.current_size_bytes <= max_size_bytes {
            break;
        }
        if index.entries.remove(&key).is_some() {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(entry.body_size_bytes);
            evicted.push(entry.hash);
        }
    }
    evicted
}

pub(super) fn remove_index_entry(zone: &CacheZoneRuntime, key: &str) {
    let mut index = lock_index(&zone.index);
    if let Some(entry) = index.entries.remove(key) {
        index.current_size_bytes = index.current_size_bytes.saturating_sub(entry.body_size_bytes);
    }
}

fn duration_to_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

pub(super) fn lock_index(mutex: &Mutex<CacheIndex>) -> std::sync::MutexGuard<'_, CacheIndex> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
