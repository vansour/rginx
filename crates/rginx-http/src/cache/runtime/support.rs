use http::StatusCode;
use http::header::{ETAG, HeaderMap, HeaderValue, LAST_MODIFIED};
use tokio::fs;

use super::*;

pub(in crate::cache) async fn remove_cache_entry_if_matches(
    zone: &Arc<CacheZoneRuntime>,
    key: &str,
    expected_entry: &CacheIndexEntry,
) -> bool {
    let _io_guard = zone.io_write(&expected_entry.hash).await;
    let Some(removed) = remove_zone_index_entry_if_matches(zone, key, expected_entry).await else {
        return false;
    };
    if removed.delete_files {
        remove_cache_files(zone.config.as_ref(), &removed.hash).await;
    }
    true
}

pub(in crate::cache) async fn remove_cache_files_if_unreferenced(
    zone: &CacheZoneRuntime,
    hash: &str,
) -> bool {
    let _io_guard = zone.io_write(hash).await;
    let referenced = {
        let index = lock_index(&zone.index);
        index.entries.values().any(|entry| entry.hash == hash)
    };
    if referenced {
        return false;
    }
    remove_cache_files(zone.config.as_ref(), hash).await;
    true
}

pub(in crate::cache) fn build_conditional_headers(
    headers: &HeaderMap,
) -> Option<CacheConditionalHeaders> {
    let if_none_match =
        header_value(headers, ETAG).and_then(|value| HeaderValue::from_str(&value).ok());
    let if_modified_since =
        header_value(headers, LAST_MODIFIED).and_then(|value| HeaderValue::from_str(&value).ok());
    (if_none_match.is_some() || if_modified_since.is_some())
        .then_some(CacheConditionalHeaders { if_none_match, if_modified_since })
}

pub(super) fn stale_if_error_window_open(entry: &CacheIndexEntry, now: u64) -> bool {
    entry.stale_if_error_until_unix_ms.is_some_and(|until| now <= until)
}

pub(super) fn use_stale_matches_status(
    conditions: &[rginx_core::CacheUseStaleCondition],
    status: StatusCode,
) -> bool {
    match status {
        StatusCode::INTERNAL_SERVER_ERROR => {
            conditions.contains(&rginx_core::CacheUseStaleCondition::Http500)
        }
        StatusCode::BAD_GATEWAY => {
            conditions.contains(&rginx_core::CacheUseStaleCondition::Http502)
        }
        StatusCode::SERVICE_UNAVAILABLE => {
            conditions.contains(&rginx_core::CacheUseStaleCondition::Http503)
        }
        StatusCode::GATEWAY_TIMEOUT => {
            conditions.contains(&rginx_core::CacheUseStaleCondition::Http504)
        }
        _ => false,
    }
}

#[derive(Debug, Clone)]
pub(in crate::cache) enum PurgeSelector {
    All,
    Exact(String),
    Prefix(String),
}

async fn remove_cache_files(zone: &rginx_core::CacheZone, hash: &str) {
    let paths = cache_paths_for_zone(zone, hash);
    let _ = fs::remove_file(paths.metadata).await;
    let _ = fs::remove_file(paths.body).await;
}
