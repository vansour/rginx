use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use http::StatusCode;
use http_body_util::BodyExt;
use tokio::fs;

use crate::handler::HttpResponse;

use super::entry::{
    CacheMetadataInput, build_cached_response_for_request, cache_key_hash, cache_metadata,
    cache_paths_for_zone, cache_variant_key, finalize_response_for_request, unix_time_ms,
    write_cache_entry, write_cache_metadata,
};
use super::policy::{
    ResponseBodySize, response_freshness, response_is_storable, response_is_storable_with_size,
    response_no_cache,
};
use super::{
    CacheIndex, CacheIndexEntry, CachePurgeResult, CacheStatus, CacheStoreContext,
    CacheZoneRuntime, persist_zone_shared_index, with_cache_status,
};

mod helpers;
mod maintenance;

use helpers::{
    cache_metadata_input, cache_vary_values, freshness_is_cacheable, merge_not_modified_headers,
};
pub(in crate::cache) use helpers::{purge_scope, purge_selector_matches};
pub(super) use maintenance::{
    cleanup_inactive_entries_in_zone, eviction_candidates, lock_index, purge_zone_entries,
    record_cache_admission_attempt, remove_index_entry,
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
    let needs_downstream_range_trim =
        super::request::cacheable_range_request(&context.request, &context.policy)
            .is_some_and(|range| range.needs_downstream_trimming());
    let storable = response_is_storable(&context, &response);
    let no_cache = response_no_cache(&context, response.status());
    if !needs_downstream_range_trim && !storable {
        return Ok(response);
    }
    if !needs_downstream_range_trim && no_cache {
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
    let downstream_response = || {
        finalize_response_for_request(
            parts.status,
            &parts.headers,
            collected.as_ref(),
            &context.request,
            &context.policy,
        )
        .map_err(|error| CacheStoreError { source: Box::new(error) })
    };

    if !storable || no_cache || collected.len() > context.zone.config.max_entry_bytes {
        return downstream_response();
    }

    let now = unix_time_ms(SystemTime::now());
    let freshness = response_freshness(&context, parts.status, &parts.headers);
    if !freshness_is_cacheable(&freshness) {
        return downstream_response();
    }

    let vary = cache_vary_values(&context, &context.request, &parts.headers);
    let final_key = cache_variant_key(&context.base_key, &vary);
    let admitted =
        record_cache_admission_attempt(context.zone.as_ref(), &final_key, context.policy.min_uses);
    persist_zone_shared_index(&context.zone).await;
    if !admitted {
        return downstream_response();
    }
    let metadata = cache_metadata(
        final_key.clone(),
        parts.status,
        &parts.headers,
        cache_metadata_input(&context.base_key, vary.clone(), now, &freshness, collected.len()),
    );
    let hash = context
        .cached_entry
        .as_ref()
        .filter(|_| context.key == final_key)
        .map(|entry| entry.hash.clone())
        .unwrap_or_else(|| cache_key_hash(&final_key));
    let paths = cache_paths_for_zone(context.zone.config.as_ref(), &hash);
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
            final_key.clone(),
            CacheIndexEntry {
                hash,
                base_key: context.base_key.clone(),
                vary,
                body_size_bytes: metadata.body_size_bytes,
                expires_at_unix_ms: metadata.expires_at_unix_ms,
                stale_if_error_until_unix_ms: metadata.stale_if_error_until_unix_ms,
                stale_while_revalidate_until_unix_ms: metadata.stale_while_revalidate_until_unix_ms,
                must_revalidate: metadata.must_revalidate,
                last_access_unix_ms: now,
            },
            context
                .cached_entry
                .as_ref()
                .filter(|_| context.key != final_key)
                .map(|entry| (context.key.clone(), entry.clone())),
        )
        .await;
    }

    downstream_response()
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
    let paths = cache_paths_for_zone(context.zone.config.as_ref(), &cached_entry.hash);
    if response_no_cache(&context, cached_status) {
        let response_metadata = cache_metadata(
            cached_metadata.key.clone(),
            cached_status,
            &merged_headers,
            CacheMetadataInput {
                base_key: cached_metadata.base_key.clone(),
                vary: cached_entry.vary.clone(),
                stored_at_unix_ms: cached_metadata.stored_at_unix_ms,
                expires_at_unix_ms: cached_metadata.expires_at_unix_ms,
                stale_if_error_until_unix_ms: cached_metadata.stale_if_error_until_unix_ms,
                stale_while_revalidate_until_unix_ms: cached_metadata
                    .stale_while_revalidate_until_unix_ms,
                must_revalidate: cached_metadata.must_revalidate,
                body_size_bytes: cached_metadata.body_size_bytes,
            },
        );
        let refreshed = {
            let _io_guard = context.zone.io_lock.lock().await;
            build_cached_response_for_request(
                &paths.body,
                &response_metadata,
                &context.request,
                &context.policy,
                context.read_cached_body,
            )
            .await
        };
        return refreshed
            .map(|response| with_cache_status(response, CacheStatus::Revalidated))
            .map_err(|error| CacheStoreError { source: Box::new(error) });
    }

    let freshness = response_freshness(&context, cached_status, &merged_headers);
    let vary = cache_vary_values(&context, &context.request, &merged_headers);
    let final_key = cache_variant_key(&context.base_key, &vary);
    let metadata = cache_metadata(
        final_key.clone(),
        cached_status,
        &merged_headers,
        cache_metadata_input(
            &context.base_key,
            vary.clone(),
            now,
            &freshness,
            cached_metadata.body_size_bytes,
        ),
    );
    if !response_is_storable_with_size(
        &context,
        cached_status,
        &merged_headers,
        ResponseBodySize::exact(cached_metadata.body_size_bytes),
    ) || !freshness_is_cacheable(&freshness)
        || final_key != context.key
    {
        let refreshed = {
            let _io_guard = context.zone.io_lock.lock().await;
            build_cached_response_for_request(
                &paths.body,
                &metadata,
                &context.request,
                &context.policy,
                context.read_cached_body,
            )
            .await
            .map_err(|error| CacheStoreError { source: Box::new(error) })
        };
        remove_index_entry(&context.zone, &context.key);
        {
            let _io_guard = context.zone.io_lock.lock().await;
            let _ = fs::remove_file(&paths.metadata).await;
            let _ = fs::remove_file(&paths.body).await;
        }
        persist_zone_shared_index(&context.zone).await;
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
            base_key: context.base_key.clone(),
            vary,
            body_size_bytes: metadata.body_size_bytes,
            expires_at_unix_ms: metadata.expires_at_unix_ms,
            stale_if_error_until_unix_ms: metadata.stale_if_error_until_unix_ms,
            stale_while_revalidate_until_unix_ms: metadata.stale_while_revalidate_until_unix_ms,
            must_revalidate: metadata.must_revalidate,
            last_access_unix_ms: now,
        },
        None,
    )
    .await;

    let refreshed = {
        let _io_guard = context.zone.io_lock.lock().await;
        build_cached_response_for_request(
            &paths.body,
            &metadata,
            &context.request,
            &context.policy,
            context.read_cached_body,
        )
        .await
        .map_err(|error| CacheStoreError { source: Box::new(error) })?
    };
    Ok(with_cache_status(refreshed, CacheStatus::Revalidated))
}

async fn update_index_after_store(
    zone: &Arc<CacheZoneRuntime>,
    key: String,
    entry: CacheIndexEntry,
    replaced_entry: Option<(String, CacheIndexEntry)>,
) {
    maintenance::update_index_after_store(zone, key, entry, replaced_entry).await;
}

pub(super) fn duration_to_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}
