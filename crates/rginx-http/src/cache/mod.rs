//! Route-level HTTP response cache for proxied responses.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use http::header::{HeaderMap, HeaderValue};
use http::{Method, Request, StatusCode, Uri};
use rginx_core::{CacheZone, ConfigSnapshot, Error, Result, RouteCachePolicy};
use tokio::fs;
use tokio::sync::Mutex as AsyncMutex;

use crate::handler::{HttpBody, HttpResponse};

mod entry;
mod load;
mod policy;
mod request;
mod store;

#[cfg(test)]
use entry::{cache_key_hash, cache_metadata, write_cache_entry};
use entry::{cache_paths, read_cached_response, unix_time_ms};
use load::load_index_from_disk;
#[cfg(test)]
use policy::{response_is_storable, response_ttl};
use request::{cache_request_bypass, render_cache_key};
use store::{lock_index, remove_index_entry, store_response};

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
    io_lock: AsyncMutex<()>,
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
                let index = load_index_from_disk(zone.as_ref()).map_err(|error| {
                    Error::Server(format!(
                        "failed to load cache zone `{name}` index from `{}`: {error}",
                        zone.path.display()
                    ))
                })?;
                Ok((
                    name.clone(),
                    Arc::new(CacheZoneRuntime {
                        config: zone.clone(),
                        index: Mutex::new(index),
                        io_lock: AsyncMutex::new(()),
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
            tracing::warn!(
                zone = %policy.zone,
                "cache policy references unknown zone; bypassing cache"
            );
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
        let (lookup, expired_hash) = {
            let mut index = lock_index(&zone.index);
            match index.entries.get_mut(&key) {
                Some(entry) if now > entry.expires_at_unix_ms => {
                    let removed = index.entries.remove(&key);
                    if let Some(entry) = removed {
                        index.current_size_bytes =
                            index.current_size_bytes.saturating_sub(entry.body_size_bytes);
                        (Err(CacheStatus::Expired), Some(entry.hash))
                    } else {
                        (Err(CacheStatus::Expired), None)
                    }
                }
                Some(entry) => {
                    entry.last_access_unix_ms = now;
                    (Ok(entry.clone()), None)
                }
                None => (Err(CacheStatus::Miss), None),
            }
        };
        if let Some(hash) = expired_hash.as_deref() {
            remove_cache_files_if_unindexed(&zone, &key, hash).await;
        }
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

        let cached_response = {
            let _io_guard = zone.io_lock.lock().await;
            read_cached_response(&zone, &entry, request.method != Method::HEAD).await
        };
        match cached_response {
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
                remove_cache_files_if_unindexed(&zone, &key, &entry.hash).await;
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
                    crate::handler::text_response(
                        StatusCode::BAD_GATEWAY,
                        "text/plain; charset=utf-8",
                        format!("failed to read upstream response while caching: {error}\n"),
                    )
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

pub(crate) fn with_cache_status(mut response: HttpResponse, status: CacheStatus) -> HttpResponse {
    response.headers_mut().insert(CACHE_STATUS_HEADER, status.as_header_value());
    response
}

async fn remove_cache_files_if_unindexed(zone: &CacheZoneRuntime, key: &str, hash: &str) {
    let _io_guard = zone.io_lock.lock().await;
    if lock_index(&zone.index).entries.contains_key(key) {
        return;
    }
    let paths = cache_paths(&zone.config.path, hash);
    let _ = fs::remove_file(paths.metadata).await;
    let _ = fs::remove_file(paths.body).await;
}

#[cfg(test)]
mod tests;
