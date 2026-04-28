use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use http::header::HeaderName;
use http::{Method, StatusCode};
use rginx_core::{
    CacheKeyTemplate, CachePredicate, CacheStatusTtlRule, CacheUseStaleCondition, CacheZone, Error,
    Result, RouteCachePolicy,
};

use crate::model::{
    CachePredicateConfig, CacheRouteConfig, CacheUseStaleConditionConfig, CacheZoneConfig,
};

const DEFAULT_CACHE_INACTIVE_SECS: u64 = 600;
const DEFAULT_CACHE_TTL_SECS: u64 = 60;
const DEFAULT_CACHE_MAX_ENTRY_BYTES: u64 = 10 * 1024 * 1024;
const DEFAULT_CACHE_KEY: &str = "{scheme}:{host}:{uri}";
const DEFAULT_CACHE_LOCK_TIMEOUT_SECS: u64 = 5;
const DEFAULT_CACHE_LOCK_AGE_SECS: u64 = 5;

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
            let ttl_by_status = cache
                .ttl_secs_by_status
                .unwrap_or_default()
                .into_iter()
                .map(|rule| {
                    Ok(CacheStatusTtlRule {
                        statuses: dedup_preserving_order(
                            rule.statuses
                                .into_iter()
                                .map(|status| StatusCode::from_u16(status).map_err(Error::from))
                                .collect::<Result<Vec<_>>>()?,
                        ),
                        ttl: Duration::from_secs(rule.ttl_secs),
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            let key =
                CacheKeyTemplate::parse(cache.key.unwrap_or_else(|| DEFAULT_CACHE_KEY.to_string()))
                    .map_err(|error| Error::Config(error.to_string()))?;
            Ok(RouteCachePolicy {
                zone: cache.zone.trim().to_string(),
                methods,
                statuses,
                ttl_by_status,
                key,
                cache_bypass: cache.cache_bypass.map(compile_cache_predicate).transpose()?,
                no_cache: cache.no_cache.map(compile_cache_predicate).transpose()?,
                stale_if_error: cache.stale_if_error_secs.map(Duration::from_secs),
                use_stale: dedup_preserving_order(
                    cache
                        .use_stale
                        .unwrap_or_default()
                        .into_iter()
                        .map(compile_use_stale_condition)
                        .collect::<Vec<_>>(),
                ),
                background_update: cache.background_update.unwrap_or(false),
                lock_timeout: Duration::from_secs(
                    cache.lock_timeout_secs.unwrap_or(DEFAULT_CACHE_LOCK_TIMEOUT_SECS),
                ),
                lock_age: Duration::from_secs(
                    cache.lock_age_secs.unwrap_or(DEFAULT_CACHE_LOCK_AGE_SECS),
                ),
            })
        })
        .transpose()
}

fn compile_cache_predicate(predicate: CachePredicateConfig) -> Result<CachePredicate> {
    match predicate {
        CachePredicateConfig::Any(predicates) => Ok(CachePredicate::Any(
            predicates.into_iter().map(compile_cache_predicate).collect::<Result<Vec<_>>>()?,
        )),
        CachePredicateConfig::All(predicates) => Ok(CachePredicate::All(
            predicates.into_iter().map(compile_cache_predicate).collect::<Result<Vec<_>>>()?,
        )),
        CachePredicateConfig::Not(predicate) => {
            Ok(CachePredicate::Not(Box::new(compile_cache_predicate(*predicate)?)))
        }
        CachePredicateConfig::Method(method) => {
            let normalized = method.trim().to_ascii_uppercase();
            Ok(CachePredicate::Method(normalized.parse::<Method>().map_err(|error| {
                Error::Config(format!("invalid cache predicate method `{method}`: {error}"))
            })?))
        }
        CachePredicateConfig::HeaderExists(name) => {
            Ok(CachePredicate::HeaderExists(compile_header_name("cache predicate header", &name)?))
        }
        CachePredicateConfig::HeaderEquals { name, value } => Ok(CachePredicate::HeaderEquals {
            name: compile_header_name("cache predicate header", &name)?,
            value,
        }),
        CachePredicateConfig::QueryExists(name) => Ok(CachePredicate::QueryExists(name)),
        CachePredicateConfig::QueryEquals { name, value } => {
            Ok(CachePredicate::QueryEquals { name, value })
        }
        CachePredicateConfig::CookieExists(name) => Ok(CachePredicate::CookieExists(name)),
        CachePredicateConfig::CookieEquals { name, value } => {
            Ok(CachePredicate::CookieEquals { name, value })
        }
        CachePredicateConfig::Status(status) => {
            Ok(CachePredicate::Status(vec![StatusCode::from_u16(status).map_err(Error::from)?]))
        }
        CachePredicateConfig::Statuses(statuses) => {
            Ok(CachePredicate::Status(dedup_preserving_order(
                statuses
                    .into_iter()
                    .map(|status| StatusCode::from_u16(status).map_err(Error::from))
                    .collect::<Result<Vec<_>>>()?,
            )))
        }
    }
}

fn compile_header_name(scope: &str, header_name: &str) -> Result<HeaderName> {
    header_name
        .parse::<HeaderName>()
        .map_err(|error| Error::Config(format!("{scope} `{header_name}` is invalid: {error}")))
}

fn compile_use_stale_condition(condition: CacheUseStaleConditionConfig) -> CacheUseStaleCondition {
    match condition {
        CacheUseStaleConditionConfig::Error => CacheUseStaleCondition::Error,
        CacheUseStaleConditionConfig::Timeout => CacheUseStaleCondition::Timeout,
        CacheUseStaleConditionConfig::Updating => CacheUseStaleCondition::Updating,
        CacheUseStaleConditionConfig::Http500 => CacheUseStaleCondition::Http500,
        CacheUseStaleConditionConfig::Http502 => CacheUseStaleCondition::Http502,
        CacheUseStaleConditionConfig::Http503 => CacheUseStaleCondition::Http503,
        CacheUseStaleConditionConfig::Http504 => CacheUseStaleCondition::Http504,
    }
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
