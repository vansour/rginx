use http::header::{
    CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderName, SET_COOKIE,
};
use rginx_core::CacheIgnoreHeader;

use super::super::invalidation::normalize_cache_tag;
use super::super::policy::{ResponseFreshness, vary_headers};
use super::super::vary::normalized_request_header_values;
use super::super::{CachedVaryHeaderValue, PurgeSelector};
use super::*;

pub(super) fn cache_metadata_input(
    base_key: &str,
    vary: Vec<CachedVaryHeaderValue>,
    tags: Vec<String>,
    now: u64,
    grace: Option<std::time::Duration>,
    keep: Option<std::time::Duration>,
    freshness: &ResponseFreshness,
    body_size_bytes: usize,
) -> CacheMetadataInput {
    let expires_at_unix_ms = now.saturating_add(duration_to_ms(freshness.ttl));
    let stale_if_error_until_unix_ms = freshness
        .stale_if_error
        .map(|duration| expires_at_unix_ms.saturating_add(duration_to_ms(duration)));
    let stale_while_revalidate_until_unix_ms = freshness
        .stale_while_revalidate
        .map(|duration| expires_at_unix_ms.saturating_add(duration_to_ms(duration)));
    let grace_until_unix_ms = max_deadline(
        grace.map(|duration| expires_at_unix_ms.saturating_add(duration_to_ms(duration))),
        stale_while_revalidate_until_unix_ms,
    );
    let keep_until_unix_ms = {
        let configured_keep_until =
            keep.map(|duration| expires_at_unix_ms.saturating_add(duration_to_ms(duration)));
        let keep_until = max_deadline(
            configured_keep_until,
            max_deadline(grace_until_unix_ms, stale_if_error_until_unix_ms),
        );
        (configured_keep_until.is_some()
            || grace_until_unix_ms.is_some()
            || stale_if_error_until_unix_ms.is_some())
        .then_some(keep_until)
        .flatten()
    };
    CacheMetadataInput {
        kind: super::super::CacheIndexEntryKind::Response,
        base_key: base_key.to_string(),
        vary,
        tags,
        stored_at_unix_ms: now,
        expires_at_unix_ms,
        grace_until_unix_ms,
        keep_until_unix_ms,
        stale_if_error_until_unix_ms,
        stale_while_revalidate_until_unix_ms,
        requires_revalidation: freshness.requires_revalidation,
        must_revalidate: freshness.must_revalidate,
        body_size_bytes,
    }
}

fn max_deadline(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(left), None) => Some(left),
        (None, Some(right)) => Some(right),
        (None, None) => None,
    }
}

pub(super) fn cache_vary_values(
    context: &crate::cache::CacheStoreContext,
    request: &crate::cache::CacheRequest,
    headers: &HeaderMap,
) -> Vec<CachedVaryHeaderValue> {
    if context.policy.ignore_headers.contains(&CacheIgnoreHeader::Vary) {
        return Vec::new();
    }
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

pub(super) fn cache_final_key_for_response(
    context: &crate::cache::CacheStoreContext,
    request: &crate::cache::CacheRequest,
    headers: &HeaderMap,
) -> (String, Vec<CachedVaryHeaderValue>, Vec<String>) {
    let vary = cache_vary_values(context, request, headers);
    let tags = cache_tags(headers);
    let final_key = if context.policy.ignore_headers.contains(&CacheIgnoreHeader::Vary)
        || vary_headers(headers).is_some()
    {
        cache_variant_key(&context.base_key, &vary)
    } else {
        context.base_key.clone()
    };
    (final_key, vary, tags)
}

pub(super) fn freshness_is_cacheable(freshness: &ResponseFreshness) -> bool {
    !freshness.ttl.is_zero()
        || freshness.requires_revalidation
        || freshness.must_revalidate
        || freshness.stale_if_error.is_some()
        || freshness.stale_while_revalidate.is_some()
}

pub(super) fn should_remember_hit_for_pass(
    context: &crate::cache::CacheStoreContext,
    headers: &HeaderMap,
    no_cache: bool,
) -> bool {
    context.policy.pass_ttl.is_some()
        && (no_cache
            || (!context.policy.ignore_headers.contains(&CacheIgnoreHeader::CacheControl)
                && headers.get(CACHE_CONTROL).and_then(|value| value.to_str().ok()).is_some_and(
                    |value| {
                        let value = value.to_ascii_lowercase();
                        value
                            .split(',')
                            .map(str::trim)
                            .any(|token| matches!(token, "no-store" | "private"))
                    },
                ))
            || (!context.policy.ignore_headers.contains(&CacheIgnoreHeader::SetCookie)
                && headers.contains_key(SET_COOKIE))
            || (!context.policy.ignore_headers.contains(&CacheIgnoreHeader::Vary)
                && vary_headers(headers).is_none())
            || headers.get(CONTENT_TYPE).and_then(|value| value.to_str().ok()).is_some_and(
                |content_type| {
                    let mime = content_type
                        .split(';')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .to_ascii_lowercase();
                    mime.eq_ignore_ascii_case("application/grpc")
                        || mime.starts_with("application/grpc+")
                        || mime.starts_with("application/grpc-web")
                },
            ))
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

fn cache_tags(headers: &HeaderMap) -> Vec<String> {
    let mut tags = Vec::new();
    for header in [
        HeaderName::from_static("cache-tag"),
        HeaderName::from_static("surrogate-key"),
        HeaderName::from_static("x-cache-tag"),
    ] {
        for value in headers.get_all(header) {
            let Ok(value) = value.to_str() else {
                continue;
            };
            for token in
                value.split(|character: char| character == ',' || character.is_ascii_whitespace())
            {
                if let Some(tag) = normalize_cache_tag(token) {
                    tags.push(tag);
                }
            }
        }
    }
    tags.sort_unstable();
    tags.dedup();
    tags
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
