//! Route-level HTTP response cache for proxied responses.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, EXPIRES, HeaderMap,
    HeaderName, HeaderValue, RANGE, SET_COOKIE, VARY,
};
use http::{Method, Request, Response, StatusCode, Uri};
use http_body_util::BodyExt;
use hyper::body::Body as _;
use rginx_core::{
    CacheKeyRenderContext, CacheZone, ConfigSnapshot, Error, Result, RouteCachePolicy,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;

use crate::handler::{HttpBody, HttpResponse, full_body};

const CACHE_STATUS_HEADER: &str = "x-cache";

#[derive(Clone, Default)]
pub(crate) struct CacheManager {
    zones: Arc<HashMap<String, Arc<CacheZoneRuntime>>>,
}

pub(crate) struct CacheStoreContext {
    zone: Arc<CacheZoneRuntime>,
    policy: RouteCachePolicy,
    key: String,
    cache_status: CacheStatus,
    store_response: bool,
}

#[derive(Clone)]
pub(crate) struct CacheRequest {
    method: Method,
    uri: Uri,
    headers: HeaderMap,
}

pub(crate) enum CacheLookup {
    Hit(HttpResponse),
    Miss(CacheStoreContext),
    Bypass(CacheStatus),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheStatus {
    Hit,
    Miss,
    Bypass,
    Expired,
}

impl CacheStatus {
    fn as_header_value(self) -> HeaderValue {
        HeaderValue::from_static(match self {
            Self::Hit => "HIT",
            Self::Miss => "MISS",
            Self::Bypass => "BYPASS",
            Self::Expired => "EXPIRED",
        })
    }
}

struct CacheZoneRuntime {
    config: Arc<CacheZone>,
    index: Mutex<CacheIndex>,
}

#[derive(Default)]
struct CacheIndex {
    entries: HashMap<String, CacheIndexEntry>,
    current_size_bytes: usize,
}

#[derive(Clone)]
struct CacheIndexEntry {
    hash: String,
    body_size_bytes: usize,
    expires_at_unix_ms: u64,
    last_access_unix_ms: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct CacheMetadata {
    status: u16,
    headers: Vec<CachedHeader>,
    stored_at_unix_ms: u64,
    expires_at_unix_ms: u64,
    body_size_bytes: usize,
}

#[derive(Debug, Serialize, Deserialize)]
struct CachedHeader {
    name: String,
    value: Vec<u8>,
}

struct CachePaths {
    dir: PathBuf,
    metadata: PathBuf,
    body: PathBuf,
}

impl CacheManager {
    pub(crate) fn from_config(config: &ConfigSnapshot) -> Result<Self> {
        let zones = config
            .cache_zones
            .iter()
            .map(|(name, zone)| {
                std::fs::create_dir_all(&zone.path).map_err(|error| {
                    Error::Server(format!(
                        "failed to create cache zone `{name}` directory `{}`: {error}",
                        zone.path.display()
                    ))
                })?;
                Ok((
                    name.clone(),
                    Arc::new(CacheZoneRuntime {
                        config: zone.clone(),
                        index: Mutex::new(CacheIndex::default()),
                    }),
                ))
            })
            .collect::<Result<HashMap<_, _>>>()?;

        Ok(Self { zones: Arc::new(zones) })
    }

    pub(crate) async fn lookup(
        &self,
        request: CacheRequest,
        downstream_scheme: &str,
        policy: &RouteCachePolicy,
    ) -> CacheLookup {
        let Some(zone) = self.zones.get(&policy.zone).cloned() else {
            return CacheLookup::Bypass(CacheStatus::Bypass);
        };

        if cache_request_bypass(&request, policy) {
            return CacheLookup::Bypass(CacheStatus::Bypass);
        }

        let key = render_cache_key(
            &request.method,
            &request.uri,
            &request.headers,
            downstream_scheme,
            policy,
        );
        let now = unix_time_ms(SystemTime::now());
        let lookup = {
            let mut index = lock_index(&zone.index);
            match index.entries.get_mut(&key) {
                Some(entry) if now > entry.expires_at_unix_ms => Err(CacheStatus::Expired),
                Some(entry) => {
                    entry.last_access_unix_ms = now;
                    Ok(entry.clone())
                }
                None => Err(CacheStatus::Miss),
            }
        };
        let entry = match lookup {
            Ok(entry) => entry,
            Err(status) => {
                return CacheLookup::Miss(CacheStoreContext::new(
                    zone,
                    policy.clone(),
                    key,
                    status,
                    request.method == Method::GET,
                ));
            }
        };

        match read_cached_response(&zone, &entry).await {
            Ok(mut response) => {
                response
                    .headers_mut()
                    .insert(CACHE_STATUS_HEADER, CacheStatus::Hit.as_header_value());
                CacheLookup::Hit(response)
            }
            Err(error) => {
                tracing::warn!(
                    zone = %zone.config.name,
                    key_hash = %entry.hash,
                    %error,
                    "failed to read cached response; treating as miss"
                );
                remove_index_entry(&zone, &key);
                CacheLookup::Miss(CacheStoreContext::new(
                    zone,
                    policy.clone(),
                    key,
                    CacheStatus::Miss,
                    request.method == Method::GET,
                ))
            }
        }
    }

    pub(crate) async fn store_response(
        &self,
        context: CacheStoreContext,
        response: HttpResponse,
    ) -> HttpResponse {
        let status = context.cache_status;
        let response = if context.store_response {
            match store_response(context, response).await {
                Ok(response) => response,
                Err(error) => {
                    tracing::warn!(%error, "failed to store cached response");
                    error.into_response
                }
            }
        } else {
            response
        };

        with_cache_status(response, status)
    }
}

impl CacheRequest {
    pub(crate) fn from_request(request: &Request<HttpBody>) -> Self {
        Self {
            method: request.method().clone(),
            uri: request.uri().clone(),
            headers: request.headers().clone(),
        }
    }
}

impl CacheStoreContext {
    fn new(
        zone: Arc<CacheZoneRuntime>,
        policy: RouteCachePolicy,
        key: String,
        cache_status: CacheStatus,
        store_response: bool,
    ) -> Self {
        Self { zone, policy, key, cache_status, store_response }
    }

    pub(crate) fn cache_status(&self) -> CacheStatus {
        self.cache_status
    }
}

struct CacheStoreError {
    source: Box<dyn std::error::Error + Send + Sync>,
    into_response: HttpResponse,
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

async fn store_response(
    context: CacheStoreContext,
    response: HttpResponse,
) -> std::result::Result<HttpResponse, CacheStoreError> {
    if !response_is_storable(&context, &response) {
        return Ok(response);
    }

    let (parts, body) = response.into_parts();
    let collected = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            return Err(CacheStoreError {
                source: error,
                into_response: Response::from_parts(parts, full_body(Bytes::new())),
            });
        }
    };

    if collected.len() > context.zone.config.max_entry_bytes {
        return Ok(Response::from_parts(parts, full_body(collected)));
    }

    let now = unix_time_ms(SystemTime::now());
    let ttl = response_ttl(&parts.headers, context.zone.config.default_ttl);
    if ttl.is_zero() {
        return Ok(Response::from_parts(parts, full_body(collected)));
    }
    let expires_at_unix_ms = now.saturating_add(duration_to_ms(ttl));
    let metadata = CacheMetadata {
        status: parts.status.as_u16(),
        headers: cached_headers(&parts.headers),
        stored_at_unix_ms: now,
        expires_at_unix_ms,
        body_size_bytes: collected.len(),
    };
    let hash = cache_key_hash(&context.key);
    let paths = cache_paths(&context.zone.config.path, &hash);

    if let Err(error) = write_cache_entry(&paths, &metadata, &collected).await {
        tracing::warn!(
            zone = %context.zone.config.name,
            key_hash = %hash,
            %error,
            "failed to write cache entry"
        );
    } else {
        update_index_after_store(
            &context.zone,
            context.key,
            CacheIndexEntry {
                hash,
                body_size_bytes: metadata.body_size_bytes,
                expires_at_unix_ms,
                last_access_unix_ms: now,
            },
        )
        .await;
    }

    Ok(Response::from_parts(parts, full_body(collected)))
}

pub(crate) fn with_cache_status(mut response: HttpResponse, status: CacheStatus) -> HttpResponse {
    response.headers_mut().insert(CACHE_STATUS_HEADER, status.as_header_value());
    response
}

fn cache_request_bypass(request: &CacheRequest, policy: &RouteCachePolicy) -> bool {
    if !policy.methods.iter().any(|method| method == &request.method) {
        return true;
    }

    if !matches!(request.method, Method::GET | Method::HEAD) {
        return true;
    }

    if request.headers.contains_key(AUTHORIZATION) || request.headers.contains_key(RANGE) {
        return true;
    }

    request.headers.get(CONTENT_TYPE).and_then(|value| value.to_str().ok()).is_some_and(
        |content_type| {
            let mime = content_type.split(';').next().unwrap_or_default().trim();
            mime.eq_ignore_ascii_case("application/grpc")
                || mime.starts_with("application/grpc+")
                || mime.starts_with("application/grpc-web")
        },
    )
}

fn render_cache_key(
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    scheme: &str,
    policy: &RouteCachePolicy,
) -> String {
    let request_uri = uri.path_and_query().map(|value| value.as_str()).unwrap_or("/");
    let host = headers
        .get(http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| uri.authority().map(|authority| authority.as_str()))
        .unwrap_or("-");
    policy.key.render(&CacheKeyRenderContext {
        scheme,
        host,
        uri: request_uri,
        method: method.as_str(),
    })
}

fn response_is_storable(context: &CacheStoreContext, response: &HttpResponse) -> bool {
    if !context.policy.statuses.iter().any(|status| *status == response.status()) {
        return false;
    }
    if response.status() == StatusCode::PARTIAL_CONTENT
        || response.headers().contains_key(CONTENT_RANGE)
        || response.headers().contains_key(SET_COOKIE)
        || response.headers().contains_key(VARY)
    {
        return false;
    }
    if response_is_grpc(response.headers()) {
        return false;
    }
    if cache_control_contains(response.headers(), &["no-store", "private", "no-cache"]) {
        return false;
    }
    if let Some(length) = parse_content_length(response.headers())
        && length > context.zone.config.max_entry_bytes
    {
        return false;
    }
    if let Some(exact) = response.body().size_hint().exact()
        && exact > context.zone.config.max_entry_bytes as u64
    {
        return false;
    }
    true
}

fn response_is_grpc(headers: &HeaderMap) -> bool {
    headers.get(CONTENT_TYPE).and_then(|value| value.to_str().ok()).is_some_and(|content_type| {
        let mime = content_type.split(';').next().unwrap_or_default().trim();
        mime.eq_ignore_ascii_case("application/grpc")
            || mime.starts_with("application/grpc+")
            || mime.starts_with("application/grpc-web")
    })
}

fn response_ttl(headers: &HeaderMap, default_ttl: Duration) -> Duration {
    cache_control_max_age(headers).or_else(|| expires_ttl(headers)).unwrap_or(default_ttl)
}

fn cache_control_max_age(headers: &HeaderMap) -> Option<Duration> {
    let mut max_age = None;
    for value in headers.get_all(CACHE_CONTROL) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for directive in value.split(',').map(str::trim) {
            let Some((name, value)) = directive.split_once('=') else {
                continue;
            };
            if name.trim().eq_ignore_ascii_case("s-maxage")
                || name.trim().eq_ignore_ascii_case("max-age")
            {
                let seconds = value.trim().trim_matches('"').parse::<u64>().ok()?;
                let duration = Duration::from_secs(seconds);
                if name.trim().eq_ignore_ascii_case("s-maxage") {
                    return Some(duration);
                }
                if max_age.is_none() {
                    max_age = Some(duration);
                }
            }
        }
    }
    max_age
}

fn expires_ttl(headers: &HeaderMap) -> Option<Duration> {
    let expires = headers.get(EXPIRES)?.to_str().ok()?;
    let expires = httpdate::parse_http_date(expires).ok()?;
    Some(expires.duration_since(SystemTime::now()).unwrap_or(Duration::ZERO))
}

fn cache_control_contains(headers: &HeaderMap, directives: &[&str]) -> bool {
    headers.get_all(CACHE_CONTROL).iter().any(|value| {
        value.to_str().ok().is_some_and(|value| {
            value.split(',').any(|directive| {
                let name = directive.split_once('=').map_or(directive, |(name, _)| name).trim();
                directives.iter().any(|expected| name.eq_ignore_ascii_case(expected))
            })
        })
    })
}

fn parse_content_length(headers: &HeaderMap) -> Option<usize> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
}

fn cached_headers(headers: &HeaderMap) -> Vec<CachedHeader> {
    headers
        .iter()
        .filter(|(name, _)| name.as_str() != CACHE_STATUS_HEADER)
        .map(|(name, value)| CachedHeader {
            name: name.as_str().to_string(),
            value: value.as_bytes().to_vec(),
        })
        .collect()
}

async fn read_cached_response(
    zone: &CacheZoneRuntime,
    entry: &CacheIndexEntry,
) -> std::io::Result<HttpResponse> {
    let paths = cache_paths(&zone.config.path, &entry.hash);
    let metadata = fs::read(&paths.metadata).await?;
    let metadata: CacheMetadata = serde_json::from_slice(&metadata)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let body = fs::read(&paths.body).await?;

    if body.len() != metadata.body_size_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "cached body length does not match metadata",
        ));
    }

    let status = StatusCode::from_u16(metadata.status)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let mut response = Response::builder().status(status);
    let headers = response.headers_mut().expect("response builder should expose headers");
    for header in metadata.headers {
        let name = HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let value = HeaderValue::from_bytes(&header.value)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        headers.append(name, value);
    }

    response
        .body(full_body(Bytes::from(body)))
        .map_err(|error| std::io::Error::other(error.to_string()))
}

async fn write_cache_entry(
    paths: &CachePaths,
    metadata: &CacheMetadata,
    body: &[u8],
) -> std::io::Result<()> {
    fs::create_dir_all(&paths.dir).await?;
    let metadata_tmp = paths.metadata.with_extension("meta.json.tmp");
    let body_tmp = paths.body.with_extension("body.tmp");
    let metadata_bytes =
        serde_json::to_vec(metadata).map_err(|error| std::io::Error::other(error.to_string()))?;

    fs::write(&body_tmp, body).await?;
    fs::write(&metadata_tmp, metadata_bytes).await?;
    fs::rename(&body_tmp, &paths.body).await?;
    fs::rename(&metadata_tmp, &paths.metadata).await?;
    Ok(())
}

async fn update_index_after_store(
    zone: &Arc<CacheZoneRuntime>,
    key: String,
    entry: CacheIndexEntry,
) {
    let evictions = {
        let mut index = lock_index(&zone.index);
        if let Some(existing) = index.entries.insert(key, entry.clone()) {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(existing.body_size_bytes);
        }
        index.current_size_bytes = index.current_size_bytes.saturating_add(entry.body_size_bytes);
        eviction_candidates(&mut index, zone.config.max_size_bytes)
    };

    for hash in evictions {
        let paths = cache_paths(&zone.config.path, &hash);
        let _ = fs::remove_file(paths.metadata).await;
        let _ = fs::remove_file(paths.body).await;
    }
}

fn eviction_candidates(index: &mut CacheIndex, max_size_bytes: Option<usize>) -> Vec<String> {
    let Some(max_size_bytes) = max_size_bytes else {
        return Vec::new();
    };
    if index.current_size_bytes <= max_size_bytes {
        return Vec::new();
    }

    let mut entries =
        index.entries.iter().map(|(key, entry)| (key.clone(), entry.clone())).collect::<Vec<_>>();
    entries.sort_by_key(|(_, entry)| entry.last_access_unix_ms);

    let mut evicted = Vec::new();
    for (key, entry) in entries {
        if index.current_size_bytes <= max_size_bytes {
            break;
        }
        if index.entries.remove(&key).is_some() {
            index.current_size_bytes =
                index.current_size_bytes.saturating_sub(entry.body_size_bytes);
            evicted.push(entry.hash);
        }
    }
    evicted
}

fn remove_index_entry(zone: &CacheZoneRuntime, key: &str) {
    let mut index = lock_index(&zone.index);
    if let Some(entry) = index.entries.remove(key) {
        index.current_size_bytes = index.current_size_bytes.saturating_sub(entry.body_size_bytes);
    }
}

fn cache_paths(base: &Path, hash: &str) -> CachePaths {
    let prefix = hash.get(..2).unwrap_or("00");
    let dir = base.join(prefix);
    CachePaths {
        metadata: dir.join(format!("{hash}.meta.json")),
        body: dir.join(format!("{hash}.body")),
        dir,
    }
}

fn cache_key_hash(key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn unix_time_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn duration_to_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn lock_index(mutex: &Mutex<CacheIndex>) -> std::sync::MutexGuard<'_, CacheIndex> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests;
