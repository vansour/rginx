use std::collections::HashSet;

use rginx_core::{Error, Result};

use crate::model::{CacheRouteConfig, CacheZoneConfig};

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
        if statuses.is_empty() {
            return Err(Error::Config(format!(
                "{route_scope} cache.statuses must not be empty when provided"
            )));
        }
        for status in statuses {
            if !(100..=599).contains(status) {
                return Err(Error::Config(format!(
                    "{route_scope} cache status `{status}` must be between 100 and 599"
                )));
            }
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

    if cache.stale_if_error_secs.is_some_and(|value| value == 0) {
        return Err(Error::Config(format!(
            "{route_scope} cache.stale_if_error_secs must be greater than 0"
        )));
    }

    Ok(())
}

fn validate_positive_optional(zone: &str, field: &str, value: Option<u64>) -> Result<()> {
    if value.is_some_and(|value| value == 0) {
        return Err(Error::Config(format!("cache zone `{zone}` {field} must be greater than 0")));
    }
    Ok(())
}
