use std::fmt::Write as _;

use http::header::RANGE;

use super::super::runtime::{CacheEntryLifecyclePhase, lifecycle_phase};
use super::super::vary::matches_vary_headers;
use super::super::{CacheIndex, CacheIndexEntry, CacheRequest, RouteCachePolicy};

pub(super) fn matching_variant_key(
    index: &CacheIndex,
    base_key: &str,
    request: &CacheRequest,
) -> Option<String> {
    if !index.variants.contains_key(base_key)
        && index
            .entries
            .get(base_key)
            .is_some_and(|entry| matches_vary_headers(request, &entry.vary))
    {
        return Some(base_key.to_string());
    }
    index
        .variants
        .get(base_key)
        .into_iter()
        .flatten()
        .find(|candidate_key| {
            index
                .entries
                .get(*candidate_key)
                .is_some_and(|entry| matches_vary_headers(request, &entry.vary))
        })
        .cloned()
}

pub(super) fn stale_allowed_for_entry(
    policy: &RouteCachePolicy,
    entry: &CacheIndexEntry,
    now: u64,
    request_forces_revalidation: bool,
) -> bool {
    lifecycle_phase(entry, now) == CacheEntryLifecyclePhase::Grace
        && !entry.is_hit_for_pass()
        && !request_forces_revalidation
        && !entry.requires_revalidation
        && !entry.must_revalidate
        && (policy.use_stale.contains(&rginx_core::CacheUseStaleCondition::Updating)
            || entry.stale_while_revalidate_until_unix_ms.is_some_and(|until| now <= until))
}

pub(super) fn fill_share_fingerprint(request: &CacheRequest) -> String {
    let mut headers = request
        .headers
        .iter()
        .filter(|(name, _)| *name != RANGE)
        .map(|(name, value)| (name.as_str().to_ascii_lowercase(), value.as_bytes().to_vec()))
        .collect::<Vec<_>>();
    headers.sort_unstable();

    let mut fingerprint = format!("method={}\n", request.method.as_str());
    for (name, value) in headers {
        fingerprint.push_str(&name);
        fingerprint.push('=');
        for byte in value {
            let _ = write!(&mut fingerprint, "{byte:02x}");
        }
        fingerprint.push('\n');
    }
    fingerprint
}
