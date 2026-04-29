use std::time::{Duration, SystemTime};

use http::header::{CACHE_CONTROL, EXPIRES, HeaderMap, PRAGMA};

pub(in crate::cache) fn cache_control_max_age(headers: &HeaderMap) -> Option<Duration> {
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
                let Ok(seconds) = value.trim().trim_matches('"').parse::<u64>() else {
                    continue;
                };
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

pub(in crate::cache) fn expires_ttl(headers: &HeaderMap) -> Option<Duration> {
    let expires = headers.get(EXPIRES)?.to_str().ok()?;
    let expires = httpdate::parse_http_date(expires).ok()?;
    Some(expires.duration_since(SystemTime::now()).unwrap_or(Duration::ZERO))
}

pub(in crate::cache) fn cache_control_contains(headers: &HeaderMap, directives: &[&str]) -> bool {
    headers.get_all(CACHE_CONTROL).iter().any(|value| {
        value.to_str().ok().is_some_and(|value| {
            value.split(',').any(|directive| {
                let name = directive.split_once('=').map_or(directive, |(name, _)| name).trim();
                directives.iter().any(|expected| name.eq_ignore_ascii_case(expected))
            })
        })
    })
}

pub(in crate::cache) fn pragma_contains(headers: &HeaderMap, directive: &str) -> bool {
    headers.get_all(PRAGMA).iter().any(|value| {
        value.to_str().ok().is_some_and(|value| {
            value.split(',').map(str::trim).any(|token| token.eq_ignore_ascii_case(directive))
        })
    })
}

pub(in crate::cache) fn cache_control_duration(
    headers: &HeaderMap,
    directive_name: &str,
) -> Option<Duration> {
    for value in headers.get_all(CACHE_CONTROL) {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for directive in value.split(',').map(str::trim) {
            let Some((name, value)) = directive.split_once('=') else {
                continue;
            };
            if !name.trim().eq_ignore_ascii_case(directive_name) {
                continue;
            }
            let Ok(seconds) = value.trim().trim_matches('"').parse::<u64>() else {
                continue;
            };
            return Some(Duration::from_secs(seconds));
        }
    }
    None
}

pub(in crate::cache) fn x_accel_expires_ttl(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get("x-accel-expires")?.to_str().ok()?.trim();
    if value == "0" {
        return Some(Duration::ZERO);
    }
    if let Some(value) = value.strip_prefix('@') {
        let seconds = value.parse::<u64>().ok()?;
        let expires_at = std::time::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))?;
        return Some(expires_at.duration_since(SystemTime::now()).unwrap_or(Duration::ZERO));
    }
    value.parse::<u64>().ok().map(Duration::from_secs)
}
