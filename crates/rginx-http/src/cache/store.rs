use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::handler::{HttpBody, HttpResponse};

use super::entry::{
    CacheMetadataInput, build_cached_response_for_request, cache_entry_temp_body_path,
    cache_key_hash, cache_metadata, cache_paths_for_zone, cache_variant_key,
    commit_cache_entry_temp_body, downstream_range_trim_plan, unix_time_ms, write_cache_metadata,
};
use super::policy::{
    ResponseBodySize, response_freshness, response_is_storable, response_is_storable_with_size,
    response_no_cache,
};
use super::{
    CacheIndex, CacheIndexEntry, CachePurgeResult, CacheStatus, CacheStoreContext,
    CacheZoneRuntime, remove_cache_files_if_unreferenced, remove_cache_files_locked,
    with_cache_status,
};

mod helpers;
mod maintenance;
pub(in crate::cache) mod range;
mod revalidate;
mod streaming;

use helpers::{
    cache_metadata_input, cache_vary_values, freshness_is_cacheable, merge_not_modified_headers,
};
pub(in crate::cache) use helpers::{purge_scope, purge_selector_matches};
pub(super) use maintenance::{
    cleanup_inactive_entries_in_zone, eviction_candidates, lock_index, purge_zone_entries,
    record_cache_admission_attempt, remove_zone_index_entry_if_matches,
};
pub(in crate::cache) use revalidate::refresh_not_modified_response;
use streaming::store_streaming_response;

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
    let downstream_range_trim = downstream_range_trim_plan(
        response.status(),
        response.headers(),
        &context.request,
        &context.policy,
    )
    .map_err(|error| CacheStoreError { source: Box::new(error) })?;
    let storable = response_is_storable(&context, &response);
    let no_cache = response_no_cache(&context, response.status());
    if downstream_range_trim.is_none() && (!storable || no_cache) {
        return Ok(response);
    }

    let (parts, body) = response.into_parts();
    Ok(store_streaming_response(context, parts, body, storable, no_cache, downstream_range_trim)
        .await)
}

async fn update_index_after_store(
    zone: &Arc<CacheZoneRuntime>,
    key: String,
    entry: CacheIndexEntry,
    replaced_entry: Option<(String, CacheIndexEntry)>,
) -> std::collections::BTreeSet<String> {
    maintenance::update_index_after_store(zone, key, entry, replaced_entry).await
}

pub(super) fn duration_to_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}
