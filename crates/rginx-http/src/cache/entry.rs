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

mod response;
mod signature;
mod temp;
mod write;

use response::{CachedFileBody, cached_headers};
pub(in crate::cache) use response::{
    DownstreamRangeTrimPlan, downstream_range_trim_plan, finalize_response_for_request,
};
pub(in crate::cache) use signature::{cache_key_hash, cache_variant_key, unix_time_ms};
#[cfg(test)]
pub(in crate::cache) use write::write_cache_entry;
pub(in crate::cache) use write::{
    cache_entry_temp_body_path, commit_cache_entry_temp_body, write_cache_metadata,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CacheMetadata {
    #[serde(default)]
    pub(super) key: String,
    #[serde(default)]
    pub(super) base_key: String,
    #[serde(default)]
    pub(super) vary: Vec<CachedVaryHeader>,
    #[serde(default)]
    pub(super) tags: Vec<String>,
    pub(super) status: u16,
    pub(super) headers: Vec<CachedHeader>,
    pub(super) stored_at_unix_ms: u64,
    pub(super) expires_at_unix_ms: u64,
    #[serde(default)]
    pub(super) kind: CacheIndexEntryKind,
    #[serde(default)]
    pub(super) grace_until_unix_ms: Option<u64>,
    #[serde(default)]
    pub(super) keep_until_unix_ms: Option<u64>,
    #[serde(default)]
    pub(super) stale_if_error_until_unix_ms: Option<u64>,
    #[serde(default)]
    pub(super) stale_while_revalidate_until_unix_ms: Option<u64>,
    #[serde(default)]
    pub(super) requires_revalidation: bool,
    #[serde(default)]
    pub(super) must_revalidate: bool,
    pub(super) body_size_bytes: usize,
}

#[derive(Debug, Deserialize)]
struct RawCacheMetadata {
    #[serde(default)]
    key: String,
    #[serde(default)]
    base_key: String,
    #[serde(default)]
    vary: Vec<CachedVaryHeader>,
    #[serde(default)]
    tags: Vec<String>,
    status: u16,
    headers: Vec<CachedHeader>,
    stored_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    #[serde(default)]
    kind: CacheIndexEntryKind,
    #[serde(default)]
    grace_until_unix_ms: Option<u64>,
    #[serde(default)]
    keep_until_unix_ms: Option<u64>,
    #[serde(default)]
    stale_if_error_until_unix_ms: Option<u64>,
    #[serde(default)]
    stale_while_revalidate_until_unix_ms: Option<u64>,
    #[serde(default)]
    requires_revalidation: Option<bool>,
    #[serde(default)]
    must_revalidate: bool,
    body_size_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CachedHeader {
    name: String,
    value: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CachedVaryHeader {
    name: String,
    #[serde(default)]
    value: Option<String>,
}

pub(super) struct CachePaths {
    pub(super) dir: PathBuf,
    pub(super) metadata: PathBuf,
    pub(super) body: PathBuf,
}

#[derive(Debug, Clone)]
pub(super) struct CacheMetadataInput {
    pub(super) kind: CacheIndexEntryKind,
    pub(super) base_key: String,
    pub(super) vary: Vec<CachedVaryHeaderValue>,
    pub(super) tags: Vec<String>,
    pub(super) stored_at_unix_ms: u64,
    pub(super) expires_at_unix_ms: u64,
    pub(super) grace_until_unix_ms: Option<u64>,
    pub(super) keep_until_unix_ms: Option<u64>,
    pub(super) stale_if_error_until_unix_ms: Option<u64>,
    pub(super) stale_while_revalidate_until_unix_ms: Option<u64>,
    pub(super) requires_revalidation: bool,
    pub(super) must_revalidate: bool,
    pub(super) body_size_bytes: usize,
}

pub(super) fn cache_metadata(
    key: String,
    status: StatusCode,
    headers: &HeaderMap,
    input: CacheMetadataInput,
) -> CacheMetadata {
    CacheMetadata {
        key,
        base_key: input.base_key,
        vary: input
            .vary
            .into_iter()
            .map(|header| CachedVaryHeader {
                name: header.name.as_str().to_string(),
                value: header.value,
            })
            .collect(),
        tags: input.tags,
        status: status.as_u16(),
        headers: cached_headers(headers, input.body_size_bytes),
        stored_at_unix_ms: input.stored_at_unix_ms,
        expires_at_unix_ms: input.expires_at_unix_ms,
        kind: input.kind,
        grace_until_unix_ms: input.grace_until_unix_ms,
        keep_until_unix_ms: input.keep_until_unix_ms,
        stale_if_error_until_unix_ms: input.stale_if_error_until_unix_ms,
        stale_while_revalidate_until_unix_ms: input.stale_while_revalidate_until_unix_ms,
        requires_revalidation: input.requires_revalidation,
        must_revalidate: input.must_revalidate,
        body_size_bytes: input.body_size_bytes,
    }
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
    if entry.is_hit_for_pass() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "hit-for-pass marker does not have cache metadata",
        ));
    }
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

pub(super) fn prepare_cached_response_head(
    hash: &str,
    metadata: CacheMetadata,
) -> std::io::Result<PreparedCacheResponseHead> {
    let headers = metadata.headers_map()?;
    let status = StatusCode::from_u16(metadata.status)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let conditional_headers = build_conditional_headers(&headers);
    Ok(PreparedCacheResponseHead {
        hash: hash.to_string(),
        metadata: Arc::new(metadata),
        status,
        headers,
        conditional_headers,
    })
}

pub(super) async fn read_cache_metadata(path: &Path) -> std::io::Result<CacheMetadata> {
    let metadata = fs::read(path).await?;
    let raw: RawCacheMetadata = serde_json::from_slice(&metadata)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    Ok(CacheMetadata {
        key: raw.key,
        base_key: raw.base_key,
        vary: raw.vary,
        tags: raw.tags,
        status: raw.status,
        headers: raw.headers,
        stored_at_unix_ms: raw.stored_at_unix_ms,
        expires_at_unix_ms: raw.expires_at_unix_ms,
        kind: raw.kind,
        grace_until_unix_ms: raw.grace_until_unix_ms,
        keep_until_unix_ms: raw.keep_until_unix_ms,
        stale_if_error_until_unix_ms: raw.stale_if_error_until_unix_ms,
        stale_while_revalidate_until_unix_ms: raw.stale_while_revalidate_until_unix_ms,
        requires_revalidation: raw.requires_revalidation.unwrap_or(raw.must_revalidate),
        must_revalidate: raw.must_revalidate,
        body_size_bytes: raw.body_size_bytes,
    })
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

impl CacheMetadata {
    pub(super) fn headers_map(&self) -> std::io::Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        for header in &self.headers {
            let name = HeaderName::from_bytes(header.name.as_bytes())
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            let value = HeaderValue::from_bytes(&header.value)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            headers.append(name, value);
        }
        Ok(headers)
    }
}
