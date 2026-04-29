use bytes::Bytes;
use http::header::{
    CONNECTION, CONTENT_LENGTH, CONTENT_RANGE, HeaderMap, HeaderName, HeaderValue,
    PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};
use http::{Response, StatusCode};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;

use crate::handler::{HttpResponse, full_body};

use super::{
    CACHE_STATUS_HEADER, CacheIndexEntry, CacheRequest, CacheZoneRuntime, CachedVaryHeaderValue,
};

mod response;
mod signature;
mod temp;

use response::cached_headers;
pub(in crate::cache) use response::finalize_response_for_request;
pub(in crate::cache) use signature::{cache_key_hash, cache_variant_key, unix_time_ms};
use temp::{cleanup_failed_write, next_temp_suffix, sibling_temp_path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CacheMetadata {
    #[serde(default)]
    pub(super) key: String,
    #[serde(default)]
    pub(super) base_key: String,
    #[serde(default)]
    pub(super) vary: Vec<CachedVaryHeader>,
    pub(super) status: u16,
    pub(super) headers: Vec<CachedHeader>,
    pub(super) stored_at_unix_ms: u64,
    pub(super) expires_at_unix_ms: u64,
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
    status: u16,
    headers: Vec<CachedHeader>,
    stored_at_unix_ms: u64,
    expires_at_unix_ms: u64,
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
    pub(super) base_key: String,
    pub(super) vary: Vec<CachedVaryHeaderValue>,
    pub(super) stored_at_unix_ms: u64,
    pub(super) expires_at_unix_ms: u64,
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
        status: status.as_u16(),
        headers: cached_headers(headers, input.body_size_bytes),
        stored_at_unix_ms: input.stored_at_unix_ms,
        expires_at_unix_ms: input.expires_at_unix_ms,
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
    let paths = cache_paths_for_zone(zone.config.as_ref(), &entry.hash);
    let metadata = read_cache_metadata(&paths.metadata).await?;
    if metadata.key != key {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cached metadata key mismatch: expected `{key}`, found `{}`", metadata.key),
        ));
    }
    build_cached_response_for_request(&paths.body, &metadata, request, policy, read_body).await
}

pub(super) async fn build_cached_response_for_request(
    body_path: &Path,
    metadata: &CacheMetadata,
    request: &CacheRequest,
    policy: &rginx_core::RouteCachePolicy,
    read_body: bool,
) -> std::io::Result<HttpResponse> {
    let body = if read_body {
        let body = fs::read(body_path).await?;
        if body.len() != metadata.body_size_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "cached body length does not match metadata",
            ));
        }
        body
    } else {
        Vec::new()
    };
    let status = StatusCode::from_u16(metadata.status)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    finalize_response_for_request(status, &metadata.headers_map()?, &body, request, policy)
}

pub(super) async fn read_cache_metadata(path: &Path) -> std::io::Result<CacheMetadata> {
    let metadata = fs::read(path).await?;
    let raw: RawCacheMetadata = serde_json::from_slice(&metadata)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    Ok(CacheMetadata {
        key: raw.key,
        base_key: raw.base_key,
        vary: raw.vary,
        status: raw.status,
        headers: raw.headers,
        stored_at_unix_ms: raw.stored_at_unix_ms,
        expires_at_unix_ms: raw.expires_at_unix_ms,
        stale_if_error_until_unix_ms: raw.stale_if_error_until_unix_ms,
        stale_while_revalidate_until_unix_ms: raw.stale_while_revalidate_until_unix_ms,
        requires_revalidation: raw.requires_revalidation.unwrap_or(raw.must_revalidate),
        must_revalidate: raw.must_revalidate,
        body_size_bytes: raw.body_size_bytes,
    })
}

pub(super) async fn write_cache_entry(
    paths: &CachePaths,
    metadata: &CacheMetadata,
    body: &[u8],
) -> std::io::Result<()> {
    fs::create_dir_all(&paths.dir).await?;
    let suffix = next_temp_suffix();
    let metadata_tmp = sibling_temp_path(&paths.metadata, &suffix);
    let body_tmp = sibling_temp_path(&paths.body, &suffix);
    let metadata_bytes =
        serde_json::to_vec(metadata).map_err(|error| std::io::Error::other(error.to_string()))?;

    if let Err(error) = fs::write(&body_tmp, body).await {
        cleanup_failed_write(paths, &body_tmp, &metadata_tmp, false).await;
        return Err(error);
    }
    if let Err(error) = fs::write(&metadata_tmp, metadata_bytes).await {
        cleanup_failed_write(paths, &body_tmp, &metadata_tmp, false).await;
        return Err(error);
    }
    if let Err(error) = fs::rename(&body_tmp, &paths.body).await {
        cleanup_failed_write(paths, &body_tmp, &metadata_tmp, false).await;
        return Err(error);
    }
    if let Err(error) = fs::rename(&metadata_tmp, &paths.metadata).await {
        cleanup_failed_write(paths, &body_tmp, &metadata_tmp, true).await;
        return Err(error);
    }
    Ok(())
}

pub(super) async fn write_cache_metadata(
    paths: &CachePaths,
    metadata: &CacheMetadata,
) -> std::io::Result<()> {
    fs::create_dir_all(&paths.dir).await?;
    let suffix = next_temp_suffix();
    let metadata_tmp = sibling_temp_path(&paths.metadata, &suffix);
    let metadata_bytes =
        serde_json::to_vec(metadata).map_err(|error| std::io::Error::other(error.to_string()))?;
    if let Err(error) = fs::write(&metadata_tmp, metadata_bytes).await {
        let _ = fs::remove_file(&metadata_tmp).await;
        return Err(error);
    }
    if let Err(error) = fs::rename(&metadata_tmp, &paths.metadata).await {
        let _ = fs::remove_file(&metadata_tmp).await;
        return Err(error);
    }
    Ok(())
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
