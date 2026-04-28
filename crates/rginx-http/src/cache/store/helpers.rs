use http::header::{CONTENT_LENGTH, HeaderMap};

use super::super::policy::{ResponseFreshness, vary_headers};
use super::super::vary::normalized_request_header_values;
use super::super::{CachedVaryHeaderValue, PurgeSelector};
use super::*;

pub(super) fn cache_metadata_input(
    base_key: &str,
    vary: Vec<CachedVaryHeaderValue>,
    now: u64,
    freshness: &ResponseFreshness,
    body_size_bytes: usize,
) -> CacheMetadataInput {
    CacheMetadataInput {
        base_key: base_key.to_string(),
        vary,
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

pub(super) fn cache_vary_values(
    request: &crate::cache::CacheRequest,
    headers: &HeaderMap,
) -> Vec<CachedVaryHeaderValue> {
    let mut vary = vary_headers(headers)
        .unwrap_or_default()
        .into_iter()
        .map(|name| CachedVaryHeaderValue {
            value: normalized_request_header_values(&request.headers, &name),
            name,
        })
        .collect::<Vec<_>>();
    vary.sort_by(|left, right| {
        left.name
            .as_str()
            .cmp(right.name.as_str())
            .then_with(|| left.value.as_deref().cmp(&right.value.as_deref()))
    });
    vary
}

pub(super) fn freshness_is_cacheable(freshness: &ResponseFreshness) -> bool {
    !freshness.ttl.is_zero()
        || freshness.must_revalidate
        || freshness.stale_if_error.is_some()
        || freshness.stale_while_revalidate.is_some()
}

pub(super) fn merge_not_modified_headers(
    cached: &HeaderMap,
    not_modified: &HeaderMap,
) -> HeaderMap {
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

pub(in crate::cache) fn purge_selector_matches(selector: &PurgeSelector, key: &str) -> bool {
    match selector {
        PurgeSelector::All => true,
        PurgeSelector::Exact(expected) => key == expected,
        PurgeSelector::Prefix(prefix) => key.starts_with(prefix),
    }
}

pub(in crate::cache) fn purge_scope(selector: &PurgeSelector) -> String {
    match selector {
        PurgeSelector::All => "all".to_string(),
        PurgeSelector::Exact(key) => format!("key:{key}"),
        PurgeSelector::Prefix(prefix) => format!("prefix:{prefix}"),
    }
}
