use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::handler::{HttpBody, HttpResponse};

use super::entry::{
    CacheMetadataInput, build_cached_response_for_request, cache_entry_temp_body_path,
    cache_key_hash, cache_metadata, cache_paths_for_zone, cache_variant_key,
    commit_cache_entry_temp_body, downstream_range_trim_plan, prepare_cached_response_head,
    unix_time_ms, write_cache_metadata,
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
    cache_final_key_for_response, cache_metadata_input, freshness_is_cacheable,
    merge_not_modified_headers, should_remember_hit_for_pass,
};
pub(in crate::cache) use helpers::{purge_scope, purge_selector_matches};
pub(super) use maintenance::{
    cleanup_inactive_entries_in_zone, clear_zone_invalidations, invalidate_zone_entries,
    lock_index, purge_zone_entries, read_index, record_cache_admission_attempt,
    remove_zone_index_entry_if_matches,
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

pub(in crate::cache) async fn remember_hit_for_pass(
    context: &CacheStoreContext,
    headers: &http::HeaderMap,
    now: u64,
) -> std::collections::BTreeSet<String> {
    let Some(pass_ttl) = context.policy.pass_ttl else {
        return std::collections::BTreeSet::new();
    };

    let (final_key, vary, tags) = cache_final_key_for_response(context, &context.request, headers);
    update_index_after_store(
        &context.zone,
        final_key.clone(),
        CacheIndexEntry {
            kind: super::CacheIndexEntryKind::HitForPass,
            hash: cache_key_hash(&format!("pass:{final_key}")),
            base_key: context.base_key.clone(),
            stored_at_unix_ms: now,
            vary,
            tags,
            body_size_bytes: 0,
            expires_at_unix_ms: now.saturating_add(duration_to_ms(pass_ttl)),
            grace_until_unix_ms: None,
            keep_until_unix_ms: Some(now.saturating_add(duration_to_ms(pass_ttl))),
            stale_if_error_until_unix_ms: None,
            stale_while_revalidate_until_unix_ms: None,
            requires_revalidation: false,
            must_revalidate: false,
            last_access_unix_ms: now,
        },
        context.cached_entry.as_ref().map(|entry| (context.key.clone(), entry.clone())),
    )
    .await
}

pub(super) fn duration_to_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}
