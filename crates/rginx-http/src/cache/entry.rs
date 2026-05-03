use bytes::Bytes;
use http::StatusCode;
use http::header::{
    CONNECTION, CONTENT_LENGTH, CONTENT_RANGE, HeaderMap, HeaderName, HeaderValue,
    PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{self, File};

use crate::handler::{HttpResponse, full_body};

use super::{
    CACHE_STATUS_HEADER, CacheIndexEntry, CacheIndexEntryKind, CacheRequest, CacheZoneRuntime,
    CachedVaryHeaderValue, PreparedCacheResponseHead, build_conditional_headers,
};

mod metadata;
mod response;
mod signature;
mod temp;
mod write;

pub(in crate::cache) use metadata::{
    CacheMetadata, CacheMetadataInput, CachedHeader, cache_metadata, prepare_cached_response_head,
    read_cache_metadata,
};
pub(in crate::cache) use response::CachedFileBody;
use response::cached_headers;
pub(in crate::cache) use response::{
    DownstreamRangeTrimPlan, downstream_range_trim_plan, finalize_response_for_request,
};
pub(in crate::cache) use signature::{cache_key_hash, cache_variant_key, unix_time_ms};
#[cfg(test)]
pub(in crate::cache) use write::write_cache_entry;
pub(in crate::cache) use write::{
    cache_entry_temp_body_path, commit_cache_entry_temp_body, write_cache_metadata,
};

pub(super) struct CachePaths {
    pub(super) dir: PathBuf,
    pub(super) metadata: PathBuf,
    pub(super) body: PathBuf,
}

pub(super) async fn read_cached_response_for_request(
    zone: &CacheZoneRuntime,
    key: &str,
    entry: &CacheIndexEntry,
    request: &CacheRequest,
    policy: &rginx_core::RouteCachePolicy,
    read_body: bool,
) -> std::io::Result<HttpResponse> {
    if entry.is_hit_for_pass() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "hit-for-pass marker does not have a cacheable response body",
        ));
    }
    if let Some(response_head) = zone.prepared_response_head(key, &entry.hash) {
        let _io_guard = zone.io_read(&entry.hash).await;
        let paths = cache_paths_for_zone(zone.config.as_ref(), &entry.hash);
        return build_cached_response_for_request(
            &paths.body,
            response_head.as_ref(),
            request,
            policy,
            read_body,
        )
        .await;
    }

    let _io_guard = zone.io_read(&entry.hash).await;
    let response_head = load_cached_response_head_locked(zone, key, entry).await?;
    let paths = cache_paths_for_zone(zone.config.as_ref(), &entry.hash);
    build_cached_response_for_request(
        &paths.body,
        response_head.as_ref(),
        request,
        policy,
        read_body,
    )
    .await
}

pub(super) async fn build_cached_response_for_request(
    body_path: &Path,
    response_head: &PreparedCacheResponseHead,
    request: &CacheRequest,
    policy: &rginx_core::RouteCachePolicy,
    read_body: bool,
) -> std::io::Result<HttpResponse> {
    if !read_body {
        return finalize_response_for_request(
            response_head.status,
            &response_head.headers,
            full_body(Bytes::new()),
            request,
            policy,
        );
    }

    let file = File::open(body_path).await?;
    let body_size_bytes = usize::try_from(file.metadata().await?.len())
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if body_size_bytes != response_head.metadata.body_size_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "cached body length does not match metadata",
        ));
    }
    finalize_response_for_request(
        response_head.status,
        &response_head.headers,
        CachedFileBody::new(file, response_head.metadata.body_size_bytes),
        request,
        policy,
    )
}

pub(super) async fn load_cached_response_head(
    zone: &CacheZoneRuntime,
    key: &str,
    entry: &CacheIndexEntry,
) -> std::io::Result<Arc<PreparedCacheResponseHead>> {
    if entry.is_hit_for_pass() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "hit-for-pass marker does not have cache metadata",
        ));
    }
    if let Some(response_head) = zone.prepared_response_head(key, &entry.hash) {
        return Ok(response_head);
    }

    let _io_guard = zone.io_read(&entry.hash).await;
    load_cached_response_head_locked(zone, key, entry).await
}

async fn load_cached_response_head_locked(
    zone: &CacheZoneRuntime,
    key: &str,
    entry: &CacheIndexEntry,
) -> std::io::Result<Arc<PreparedCacheResponseHead>> {
    if let Some(response_head) = zone.prepared_response_head(key, &entry.hash) {
        return Ok(response_head);
    }

    let paths = cache_paths_for_zone(zone.config.as_ref(), &entry.hash);
    let metadata = read_cache_metadata(&paths.metadata).await?;
    if metadata.key != key {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cached metadata key mismatch: expected `{key}`, found `{}`", metadata.key),
        ));
    }

    let response_head = Arc::new(prepare_cached_response_head(&entry.hash, metadata)?);
    zone.store_prepared_response_head(key, entry.last_access_unix_ms, response_head.clone());
    Ok(response_head)
}

#[cfg(test)]
pub(super) fn cache_paths(base: &Path, hash: &str) -> CachePaths {
    cache_paths_with_levels(base, &[2], hash)
}

pub(super) fn cache_paths_for_zone(zone: &rginx_core::CacheZone, hash: &str) -> CachePaths {
    cache_paths_with_levels(&zone.path, &zone.path_levels, hash)
}

fn cache_paths_with_levels(base: &Path, levels: &[usize], hash: &str) -> CachePaths {
    let mut dir = base.to_path_buf();
    let mut offset = 0usize;
    for level in levels {
        dir = dir.join(cache_path_segment(hash, offset, *level));
        offset = offset.saturating_add(*level);
    }
    CachePaths {
        metadata: dir.join(format!("{hash}.meta.json")),
        body: dir.join(format!("{hash}.body")),
        dir,
    }
}

fn cache_path_segment(hash: &str, offset: usize, level_len: usize) -> String {
    hash.get(offset..offset.saturating_add(level_len)).map(str::to_string).unwrap_or_else(|| {
        format!("{:0<width$}", hash.get(offset..).unwrap_or(""), width = level_len)
    })
}
