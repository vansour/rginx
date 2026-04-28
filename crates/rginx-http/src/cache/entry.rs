use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http::header::{
    CONNECTION, CONTENT_LENGTH, HeaderMap, HeaderName, HeaderValue, PROXY_AUTHENTICATE,
    PROXY_AUTHORIZATION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};
use http::{Response, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;

use crate::handler::{HttpResponse, full_body};

use super::{CACHE_STATUS_HEADER, CacheIndexEntry, CacheZoneRuntime};

mod temp;

use temp::{cleanup_failed_write, next_temp_suffix, sibling_temp_path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct CacheMetadata {
    #[serde(default)]
    pub(super) key: String,
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

pub(super) struct CachePaths {
    pub(super) dir: PathBuf,
    pub(super) metadata: PathBuf,
    pub(super) body: PathBuf,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct CacheMetadataInput {
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

pub(super) async fn read_cached_response(
    zone: &CacheZoneRuntime,
    key: &str,
    entry: &CacheIndexEntry,
    read_body: bool,
) -> std::io::Result<HttpResponse> {
    let paths = cache_paths(&zone.config.path, &entry.hash);
    let metadata = read_cache_metadata(&paths.metadata).await?;
    if metadata.key != key {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("cached metadata key mismatch: expected `{key}`, found `{}`", metadata.key),
        ));
    }
    build_cached_response(&paths.body, &metadata, read_body).await
}

pub(super) async fn build_cached_response(
    body_path: &Path,
    metadata: &CacheMetadata,
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
    let mut response = Response::builder().status(status);
    let headers = response.headers_mut().expect("response builder should expose headers");
    *headers = metadata.headers_map()?;

    response
        .body(full_body(Bytes::from(body)))
        .map_err(|error| std::io::Error::other(error.to_string()))
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

pub(super) fn cache_paths(base: &Path, hash: &str) -> CachePaths {
    let prefix = hash.get(..2).unwrap_or("00");
    let dir = base.join(prefix);
    CachePaths {
        metadata: dir.join(format!("{hash}.meta.json")),
        body: dir.join(format!("{hash}.body")),
        dir,
    }
}

pub(super) fn cache_key_hash(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

pub(super) fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
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
