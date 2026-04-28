use std::collections::HashSet;

use http::{HeaderName, Method};
use rginx_core::{Error, Result};

use crate::model::{CachePredicateConfig, CacheRouteConfig, CacheStatusTtlConfig, CacheZoneConfig};

pub(super) fn validate_cache_zones(zones: &[CacheZoneConfig]) -> Result<HashSet<String>> {
    let mut names = HashSet::new();

    for zone in zones {
        let name = zone.name.trim();
        if name.is_empty() {
            return Err(Error::Config("cache_zones[].name must not be empty".to_string()));
        }
        if !names.insert(name.to_string()) {
            return Err(Error::Config(format!("duplicate cache zone `{name}`")));
        }
        if zone.path.trim().is_empty() {
            return Err(Error::Config(format!("cache zone `{name}` path must not be empty")));
        }
        validate_positive_optional(name, "max_size_bytes", zone.max_size_bytes)?;
        validate_positive_optional(name, "inactive_secs", zone.inactive_secs)?;
        validate_positive_optional(name, "default_ttl_secs", zone.default_ttl_secs)?;
        validate_positive_optional(name, "max_entry_bytes", zone.max_entry_bytes)?;
        if let (Some(max_size), Some(max_entry)) = (zone.max_size_bytes, zone.max_entry_bytes)
            && max_entry > max_size
        {
            return Err(Error::Config(format!(
                "cache zone `{name}` max_entry_bytes must not exceed max_size_bytes"
            )));
        }
    }

    Ok(names)
}

pub(super) fn validate_route_cache(
    route_scope: &str,
    cache: Option<&CacheRouteConfig>,
    cache_zone_names: &HashSet<String>,
) -> Result<()> {
    let Some(cache) = cache else {
        return Ok(());
    };

    let zone = cache.zone.trim();
    if zone.is_empty() {
        return Err(Error::Config(format!("{route_scope} cache.zone must not be empty")));
    }
    if !cache_zone_names.contains(zone) {
        return Err(Error::Config(format!(
            "{route_scope} references undefined cache zone `{}`",
            zone
        )));
    }

    if let Some(methods) = cache.methods.as_deref() {
        if methods.is_empty() {
            return Err(Error::Config(format!(
                "{route_scope} cache.methods must not be empty when provided"
            )));
        }
        let mut allows_get = false;
        for method in methods {
            match method.trim().to_ascii_uppercase().as_str() {
                "GET" => allows_get = true,
                "HEAD" => {}
                other => {
                    return Err(Error::Config(format!(
                        "{route_scope} cache method `{other}` is not supported by the MVP"
                    )));
                }
            }
        }
        if !allows_get {
            return Err(Error::Config(format!(
                "{route_scope} cache.methods must include GET so responses can populate the cache"
            )));
        }
    }

    if let Some(statuses) = cache.statuses.as_deref() {
        validate_statuses(route_scope, "cache.statuses", statuses)?;
    }

    if let Some(ttl_rules) = cache.ttl_secs_by_status.as_deref() {
        if ttl_rules.is_empty() {
            return Err(Error::Config(format!(
                "{route_scope} cache.ttl_secs_by_status must not be empty when provided"
            )));
        }
        for (index, rule) in ttl_rules.iter().enumerate() {
            validate_status_ttl_rule(route_scope, index, rule)?;
        }
    }

    if let Some(key) = cache.key.as_deref() {
        if key.trim().is_empty() {
            return Err(Error::Config(format!("{route_scope} cache.key must not be empty")));
        }
        rginx_core::CacheKeyTemplate::parse(key).map_err(|error| {
            Error::Config(format!("{route_scope} cache.key is invalid: {error}"))
        })?;
    }

    if let Some(cache_bypass) = cache.cache_bypass.as_ref() {
        validate_cache_predicate(
            route_scope,
            "cache.cache_bypass",
            cache_bypass,
            PredicateValidationMode::RequestOnly,
        )?;
    }

    if let Some(no_cache) = cache.no_cache.as_ref() {
        validate_cache_predicate(
            route_scope,
            "cache.no_cache",
            no_cache,
            PredicateValidationMode::RequestOrResponse,
        )?;
    }

    if cache.stale_if_error_secs.is_some_and(|value| value == 0) {
        return Err(Error::Config(format!(
            "{route_scope} cache.stale_if_error_secs must be greater than 0"
        )));
    }

    if cache.lock_timeout_secs.is_some_and(|value| value == 0) {
        return Err(Error::Config(format!(
            "{route_scope} cache.lock_timeout_secs must be greater than 0"
        )));
    }

    if cache.lock_age_secs.is_some_and(|value| value == 0) {
        return Err(Error::Config(format!(
            "{route_scope} cache.lock_age_secs must be greater than 0"
        )));
    }

    Ok(())
}

fn validate_status_ttl_rule(
    route_scope: &str,
    index: usize,
    rule: &CacheStatusTtlConfig,
) -> Result<()> {
    let field = format!("cache.ttl_secs_by_status[{index}]");
    if rule.statuses.is_empty() {
        return Err(Error::Config(format!("{route_scope} {field}.statuses must not be empty")));
    }
    validate_statuses(route_scope, &format!("{field}.statuses"), &rule.statuses)?;
    if rule.ttl_secs == 0 {
        return Err(Error::Config(format!(
            "{route_scope} {field}.ttl_secs must be greater than 0"
        )));
    }
    Ok(())
}

fn validate_statuses(route_scope: &str, field: &str, statuses: &[u16]) -> Result<()> {
    if statuses.is_empty() {
        return Err(Error::Config(format!(
            "{route_scope} {field} must not be empty when provided"
        )));
    }
    for status in statuses {
        if !(100..=599).contains(status) {
            return Err(Error::Config(format!(
                "{route_scope} {field} status `{status}` must be between 100 and 599"
            )));
        }
    }
    Ok(())
}

fn validate_cache_predicate(
    route_scope: &str,
    field: &str,
    predicate: &CachePredicateConfig,
    mode: PredicateValidationMode,
) -> Result<()> {
    match predicate {
        CachePredicateConfig::Any(predicates) | CachePredicateConfig::All(predicates) => {
            if predicates.is_empty() {
                return Err(Error::Config(format!(
                    "{route_scope} {field} composite predicates must not be empty"
                )));
            }
            for predicate in predicates {
                validate_cache_predicate(route_scope, field, predicate, mode)?;
            }
        }
        CachePredicateConfig::Not(predicate) => {
            validate_cache_predicate(route_scope, field, predicate, mode)?;
        }
        CachePredicateConfig::Method(method) => {
            method.trim().to_ascii_uppercase().parse::<Method>().map_err(|error| {
                Error::Config(format!(
                    "{route_scope} {field} method `{method}` is invalid: {error}"
                ))
            })?;
        }
        CachePredicateConfig::HeaderExists(name)
        | CachePredicateConfig::QueryExists(name)
        | CachePredicateConfig::CookieExists(name) => {
            validate_non_empty(route_scope, field, name, "name")?;
            if matches!(predicate, CachePredicateConfig::HeaderExists(_)) {
                validate_header_name(route_scope, field, name)?;
            }
        }
        CachePredicateConfig::HeaderEquals { name, .. } => {
            validate_non_empty(route_scope, field, name, "name")?;
            validate_header_name(route_scope, field, name)?;
        }
        CachePredicateConfig::QueryEquals { name, .. }
        | CachePredicateConfig::CookieEquals { name, .. } => {
            validate_non_empty(route_scope, field, name, "name")?;
        }
        CachePredicateConfig::Status(status) => {
            validate_statuses(route_scope, field, &[*status])?;
            if mode == PredicateValidationMode::RequestOnly {
                return Err(Error::Config(format!(
                    "{route_scope} {field} cannot match response status"
                )));
            }
        }
        CachePredicateConfig::Statuses(statuses) => {
            validate_statuses(route_scope, field, statuses)?;
            if mode == PredicateValidationMode::RequestOnly {
                return Err(Error::Config(format!(
                    "{route_scope} {field} cannot match response status"
                )));
            }
        }
    }
    Ok(())
}

fn validate_non_empty(route_scope: &str, field: &str, value: &str, part: &str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(Error::Config(format!("{route_scope} {field} {part} must not be empty")));
    }
    Ok(())
}

fn validate_header_name(route_scope: &str, field: &str, name: &str) -> Result<()> {
    name.parse::<HeaderName>().map_err(|error| {
        Error::Config(format!("{route_scope} {field} header `{name}` is invalid: {error}"))
    })?;
    Ok(())
}

fn validate_positive_optional(zone: &str, field: &str, value: Option<u64>) -> Result<()> {
    if value.is_some_and(|value| value == 0) {
        return Err(Error::Config(format!("cache zone `{zone}` {field} must be greater than 0")));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PredicateValidationMode {
    RequestOnly,
    RequestOrResponse,
}
