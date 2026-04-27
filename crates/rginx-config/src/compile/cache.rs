use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use http::{Method, StatusCode};
use rginx_core::{CacheKeyTemplate, CacheZone, Error, Result, RouteCachePolicy};

use crate::model::{CacheRouteConfig, CacheZoneConfig};

const DEFAULT_CACHE_INACTIVE_SECS: u64 = 600;
const DEFAULT_CACHE_TTL_SECS: u64 = 60;
const DEFAULT_CACHE_MAX_ENTRY_BYTES: u64 = 10 * 1024 * 1024;
const DEFAULT_CACHE_KEY: &str = "{scheme}:{host}:{uri}";

pub(super) fn compile_cache_zones(
    zones: Vec<CacheZoneConfig>,
    base_dir: &Path,
) -> Result<HashMap<String, Arc<CacheZone>>> {
    zones
        .into_iter()
        .map(|zone| {
            let name = zone.name.trim().to_string();
            let path = super::path::resolve_path(base_dir, zone.path);
            let max_size_bytes = zone.max_size_bytes.map(usize_from_u64).transpose()?;
            let default_max_entry_bytes = usize_from_u64(DEFAULT_CACHE_MAX_ENTRY_BYTES)?;
            let max_entry_bytes = match zone.max_entry_bytes {
                Some(value) => usize_from_u64(value)?,
                None => max_size_bytes
                    .map(|max_size| default_max_entry_bytes.min(max_size))
                    .unwrap_or(default_max_entry_bytes),
            };
            if let Some(max_size_bytes) = max_size_bytes
                && max_entry_bytes > max_size_bytes
            {
                return Err(Error::Config(format!(
                    "cache zone `{name}` max_entry_bytes must not exceed max_size_bytes"
                )));
            }
            let compiled = CacheZone {
                name: name.clone(),
                path,
                max_size_bytes,
                inactive: Duration::from_secs(
                    zone.inactive_secs.unwrap_or(DEFAULT_CACHE_INACTIVE_SECS),
                ),
                default_ttl: Duration::from_secs(
                    zone.default_ttl_secs.unwrap_or(DEFAULT_CACHE_TTL_SECS),
                ),
                max_entry_bytes,
            };
            Ok((name, Arc::new(compiled)))
        })
        .collect()
}

pub(super) fn compile_route_cache(
    cache: Option<CacheRouteConfig>,
) -> Result<Option<RouteCachePolicy>> {
    cache
        .map(|cache| {
            let methods = dedup_preserving_order(match cache.methods {
                Some(methods) => methods
                    .into_iter()
                    .map(|method| {
                        let normalized = method.trim().to_ascii_uppercase();
                        normalized.parse::<Method>().map_err(|error| {
                            Error::Config(format!("invalid cache method `{method}`: {error}"))
                        })
                    })
                    .collect::<Result<Vec<_>>>()?,
                None => vec![Method::GET, Method::HEAD],
            });
            let statuses = dedup_preserving_order(match cache.statuses {
                Some(statuses) => statuses
                    .into_iter()
                    .map(|status| StatusCode::from_u16(status).map_err(Error::from))
                    .collect::<Result<Vec<_>>>()?,
                None => vec![
                    StatusCode::OK,
                    StatusCode::MOVED_PERMANENTLY,
                    StatusCode::FOUND,
                    StatusCode::NOT_FOUND,
                ],
            });
            let key =
                CacheKeyTemplate::parse(cache.key.unwrap_or_else(|| DEFAULT_CACHE_KEY.to_string()))
                    .map_err(|error| Error::Config(error.to_string()))?;
            Ok(RouteCachePolicy {
                zone: cache.zone.trim().to_string(),
                methods,
                statuses,
                key,
                stale_if_error: cache.stale_if_error_secs.map(Duration::from_secs),
            })
        })
        .transpose()
}

fn usize_from_u64(value: u64) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| Error::Config(format!("cache size `{value}` does not fit into usize")))
}

fn dedup_preserving_order<T: PartialEq>(values: Vec<T>) -> Vec<T> {
    let mut deduped = Vec::with_capacity(values.len());
    for value in values {
        if !deduped.contains(&value) {
            deduped.push(value);
        }
    }
    deduped
}
