use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use http::header::{CONTENT_LENGTH, HeaderMap};
use http::{Response, StatusCode};
use http_body_util::BodyExt;
use tokio::fs;

use crate::handler::{HttpResponse, full_body};

use super::entry::{
    CacheMetadataInput, build_cached_response, cache_key_hash, cache_metadata, cache_paths,
    unix_time_ms, write_cache_entry, write_cache_metadata,
};
use super::policy::{
    ResponseBodySize, ResponseFreshness, response_freshness, response_is_storable,
    response_is_storable_with_size,
};
use super::{
    CacheIndex, CacheIndexEntry, CachePurgeResult, CacheStatus, CacheStoreContext,
    CacheZoneRuntime, PurgeSelector, with_cache_status,
};

mod maintenance;

pub(super) use maintenance::{
    cleanup_inactive_entries_in_zone, eviction_candidates, lock_index, purge_zone_entries,
    remove_index_entry,
};

pub(crate) struct CacheStoreError {
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
            context.zone.record_write_error();
            return Err(CacheStoreError { source: error });
        }
    };

    if collected.len() > context.zone.config.max_entry_bytes {
        return Ok(Response::from_parts(parts, full_body(collected)));
    }

    let now = unix_time_ms(SystemTime::now());
    let freshness = response_freshness(&context, &parts.headers);
    if !freshness_is_cacheable(&freshness) {
        return Ok(Response::from_parts(parts, full_body(collected)));
    }

    let metadata = cache_metadata(
        context.key.clone(),
        parts.status,
        &parts.headers,
        cache_metadata_input(now, &freshness, collected.len()),
    );
    let hash = context
        .cached_entry
        .as_ref()
        .map(|entry| entry.hash.clone())
        .unwrap_or_else(|| cache_key_hash(&context.key));
    let paths = cache_paths(&context.zone.config.path, &hash);
    let _io_guard = context.zone.io_lock.lock().await;

    if let Err(error) = write_cache_entry(&paths, &metadata, &collected).await {
        tracing::warn!(
            zone = %context.zone.config.name,
            key_hash = %hash,
            %error,
            "failed to write cache entry"
        );
        context.zone.record_write_error();
    } else {
        context.zone.record_write_success();
        if context.revalidating {
            context.zone.record_revalidated();
        }
        update_index_after_store(
            &context.zone,
            context.key,
            CacheIndexEntry {
                hash,
                body_size_bytes: metadata.body_size_bytes,
                expires_at_unix_ms: metadata.expires_at_unix_ms,
                stale_if_error_until_unix_ms: metadata.stale_if_error_until_unix_ms,
                stale_while_revalidate_until_unix_ms: metadata.stale_while_revalidate_until_unix_ms,
                must_revalidate: metadata.must_revalidate,
                last_access_unix_ms: now,
            },
        )
        .await;
    }

    Ok(Response::from_parts(parts, full_body(collected)))
}

pub(super) async fn refresh_not_modified_response(
    context: CacheStoreContext,
    response: HttpResponse,
) -> std::result::Result<HttpResponse, CacheStoreError> {
    let Some(cached_entry) = context.cached_entry.clone() else {
        context.zone.record_write_error();
        return Err(CacheStoreError {
            source: Box::new(std::io::Error::other("missing cached entry for 304 revalidation")),
        });
    };
    let Some(cached_metadata) = context.cached_metadata.clone() else {
        context.zone.record_write_error();
        return Err(CacheStoreError {
            source: Box::new(std::io::Error::other("missing cached metadata for 304 revalidation")),
        });
    };

    let cached_headers = cached_metadata
        .headers_map()
        .map_err(|error| CacheStoreError { source: Box::new(error) })?;
    let merged_headers = merge_not_modified_headers(&cached_headers, response.headers());
    let now = unix_time_ms(SystemTime::now());
    let cached_status = StatusCode::from_u16(cached_metadata.status).unwrap_or(StatusCode::OK);
    let freshness = response_freshness(&context, &merged_headers);
    let metadata = cache_metadata(
        context.key.clone(),
        cached_status,
        &merged_headers,
        cache_metadata_input(now, &freshness, cached_metadata.body_size_bytes),
    );
    let paths = cache_paths(&context.zone.config.path, &cached_entry.hash);
    if !response_is_storable_with_size(
        &context,
        cached_status,
        &merged_headers,
        ResponseBodySize::exact(cached_metadata.body_size_bytes),
    ) || !freshness_is_cacheable(&freshness)
    {
        let refreshed = {
            let _io_guard = context.zone.io_lock.lock().await;
            build_cached_response(&paths.body, &metadata, context.read_cached_body)
                .await
                .map_err(|error| CacheStoreError { source: Box::new(error) })
        };
        remove_index_entry(&context.zone, &context.key);
        {
            let _io_guard = context.zone.io_lock.lock().await;
            let _ = fs::remove_file(&paths.metadata).await;
            let _ = fs::remove_file(&paths.body).await;
        }
        context.zone.record_revalidated();
        return refreshed.map(|response| with_cache_status(response, CacheStatus::Revalidated));
    }
    {
        let _io_guard = context.zone.io_lock.lock().await;
        write_cache_metadata(&paths, &metadata)
            .await
            .map_err(|error| CacheStoreError { source: Box::new(error) })?;
    }
    context.zone.record_write_success();
    context.zone.record_revalidated();
    update_index_after_store(
        &context.zone,
        context.key,
        CacheIndexEntry {
            hash: cached_entry.hash.clone(),
            body_size_bytes: metadata.body_size_bytes,
            expires_at_unix_ms: metadata.expires_at_unix_ms,
            stale_if_error_until_unix_ms: metadata.stale_if_error_until_unix_ms,
            stale_while_revalidate_until_unix_ms: metadata.stale_while_revalidate_until_unix_ms,
            must_revalidate: metadata.must_revalidate,
            last_access_unix_ms: now,
        },
    )
    .await;

    let refreshed = {
        let _io_guard = context.zone.io_lock.lock().await;
        build_cached_response(&paths.body, &metadata, context.read_cached_body)
            .await
            .map_err(|error| CacheStoreError { source: Box::new(error) })?
    };
    Ok(with_cache_status(refreshed, CacheStatus::Revalidated))
}

async fn update_index_after_store(
    zone: &Arc<CacheZoneRuntime>,
    key: String,
    entry: CacheIndexEntry,
) {
    maintenance::update_index_after_store(zone, key, entry).await;
}

pub(super) fn duration_to_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn cache_metadata_input(
    now: u64,
    freshness: &ResponseFreshness,
    body_size_bytes: usize,
) -> CacheMetadataInput {
    CacheMetadataInput {
        stored_at_unix_ms: now,
        expires_at_unix_ms: now.saturating_add(duration_to_ms(freshness.ttl)),
        stale_if_error_until_unix_ms: freshness
            .stale_if_error
            .map(|duration| now.saturating_add(duration_to_ms(duration))),
        stale_while_revalidate_until_unix_ms: freshness
            .stale_while_revalidate
            .map(|duration| now.saturating_add(duration_to_ms(duration))),
        must_revalidate: freshness.must_revalidate,
        body_size_bytes,
    }
}

fn freshness_is_cacheable(freshness: &ResponseFreshness) -> bool {
    !freshness.ttl.is_zero()
        || freshness.must_revalidate
        || freshness.stale_if_error.is_some()
        || freshness.stale_while_revalidate.is_some()
}

fn merge_not_modified_headers(cached: &HeaderMap, not_modified: &HeaderMap) -> HeaderMap {
    let mut merged = cached.clone();
    for name in not_modified.keys() {
        if *name == CONTENT_LENGTH {
            continue;
        }
        merged.remove(name);
        for value in not_modified.get_all(name) {
            merged.append(name.clone(), value.clone());
        }
    }
    merged
}

pub(super) fn purge_selector_matches(selector: &PurgeSelector, key: &str) -> bool {
    match selector {
        PurgeSelector::All => true,
        PurgeSelector::Exact(expected) => key == expected,
        PurgeSelector::Prefix(prefix) => key.starts_with(prefix),
    }
}

pub(super) fn purge_scope(selector: &PurgeSelector) -> String {
    match selector {
        PurgeSelector::All => "all".to_string(),
        PurgeSelector::Exact(key) => format!("key:{key}"),
        PurgeSelector::Prefix(prefix) => format!("prefix:{prefix}"),
    }
}
