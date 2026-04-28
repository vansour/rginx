use http::{HeaderName, Method};
use rginx_core::{Error, Result};

use crate::model::CachePredicateConfig;

pub(super) fn validate_cache_predicate(
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
            super::validate_statuses(route_scope, field, &[*status])?;
            if mode == PredicateValidationMode::RequestOnly {
                return Err(Error::Config(format!(
                    "{route_scope} {field} cannot match response status"
                )));
            }
        }
        CachePredicateConfig::Statuses(statuses) => {
            super::validate_statuses(route_scope, field, statuses)?;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PredicateValidationMode {
    RequestOnly,
    RequestOrResponse,
}
