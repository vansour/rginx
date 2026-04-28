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

mod signature;
mod temp;

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
    pub(super) must_revalidate: bool,
    pub(super) body_size_bytes: usize,
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

pub(super) fn finalize_response_for_request(
    status: StatusCode,
    headers: &HeaderMap,
    body: &[u8],
    request: &CacheRequest,
    policy: &rginx_core::RouteCachePolicy,
) -> std::io::Result<HttpResponse> {
    let Some(request_range) = super::request::cacheable_range_request(request, policy)
        .filter(|range| range.needs_downstream_trimming())
    else {
        return build_response(status, headers, body.to_vec());
    };

    let mut headers = headers.clone();
    let cached_range = parse_cached_content_range(&headers)?;
    if cached_range.start != request_range.cache_start {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "cached content-range start `{}` does not match expected slice start `{}`",
                cached_range.start, request_range.cache_start
            ),
        ));
    }

    let response_end = request_range.request_end.min(cached_range.end);
    if request_range.request_start < cached_range.start
        || request_range.request_start > response_end
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "requested range `{}-{}` is not satisfiable from cached slice `{}-{}`",
                request_range.request_start,
                request_range.request_end,
                cached_range.start,
                cached_range.end
            ),
        ));
    }

    let body = if body.is_empty() {
        Vec::new()
    } else {
        let start_offset = usize::try_from(request_range.request_start - cached_range.start)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let end_offset = usize::try_from(response_end - cached_range.start + 1)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        body.get(start_offset..end_offset)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "requested range exceeds cached slice body bounds",
                )
            })?
            .to_vec()
    };

    let response_len = usize::try_from(response_end - request_range.request_start + 1)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&response_len.to_string())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
    );
    headers.insert(
        CONTENT_RANGE,
        HeaderValue::from_str(&format!(
            "bytes {}-{}/{}",
            request_range.request_start,
            response_end,
            cached_range.total.map(|total| total.to_string()).unwrap_or_else(|| "*".to_string())
        ))
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
    );
    build_response(StatusCode::PARTIAL_CONTENT, &headers, body)
}

pub(super) async fn read_cache_metadata(path: &Path) -> std::io::Result<CacheMetadata> {
    let metadata = fs::read(path).await?;
    serde_json::from_slice(&metadata)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
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

struct CachedContentRange {
    start: u64,
    end: u64,
    total: Option<u64>,
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

fn parse_cached_content_range(headers: &HeaderMap) -> std::io::Result<CachedContentRange> {
    let value = headers.get(CONTENT_RANGE).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "cached slice metadata is missing Content-Range",
        )
    })?;
    let value = value
        .to_str()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let value = value.trim().strip_prefix("bytes ").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cached Content-Range `{value}` is not a byte range"),
        )
    })?;
    let (range, total) = value.split_once('/').ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cached Content-Range `{value}` is malformed"),
        )
    })?;
    let (start, end) = range.split_once('-').ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cached Content-Range `{value}` is malformed"),
        )
    })?;
    let start = start
        .trim()
        .parse::<u64>()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let end = end
        .trim()
        .parse::<u64>()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let total = match total.trim() {
        "*" => None,
        value => Some(
            value
                .parse::<u64>()
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?,
        ),
    };
    Ok(CachedContentRange { start, end, total })
}

fn build_response(
    status: StatusCode,
    headers: &HeaderMap,
    body: Vec<u8>,
) -> std::io::Result<HttpResponse> {
    let mut response = Response::builder().status(status);
    *response.headers_mut().expect("response builder should expose headers") = headers.clone();
    response
        .body(full_body(Bytes::from(body)))
        .map_err(|error| std::io::Error::other(error.to_string()))
}

fn cached_headers(headers: &HeaderMap, body_size_bytes: usize) -> Vec<CachedHeader> {
    let mut headers = headers.clone();
    let had_content_length = headers.contains_key(CONTENT_LENGTH);
    remove_cache_hop_by_hop_headers(&mut headers);
    headers.remove(CACHE_STATUS_HEADER);
    headers.remove(CONTENT_LENGTH);
    if had_content_length || body_size_bytes > 0 {
        headers.insert(
            CONTENT_LENGTH,
            HeaderValue::from_str(&body_size_bytes.to_string())
                .expect("cache body length should fit in a header"),
        );
    }

    headers
        .iter()
        .map(|(name, value)| CachedHeader {
            name: name.as_str().to_string(),
            value: value.as_bytes().to_vec(),
        })
        .collect()
}

fn remove_cache_hop_by_hop_headers(headers: &mut HeaderMap) {
    let mut extra_headers = Vec::new();
    for value in headers.get_all(CONNECTION) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for token in value.split(',').map(str::trim).filter(|token| !token.is_empty()) {
            if let Ok(name) = HeaderName::from_bytes(token.as_bytes()) {
                extra_headers.push(name);
            }
        }
    }

    for name in extra_headers {
        headers.remove(name);
    }
    for name in [
        CONNECTION,
        PROXY_AUTHENTICATE,
        PROXY_AUTHORIZATION,
        TE,
        TRAILER,
        TRANSFER_ENCODING,
        UPGRADE,
    ] {
        headers.remove(name);
    }
    headers.remove("keep-alive");
    headers.remove("proxy-connection");
}
