use std::time::{Duration, SystemTime};

use http::StatusCode;
use http::header::{
    CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, EXPIRES, HeaderMap, SET_COOKIE,
    VARY,
};
use hyper::body::Body as _;

use crate::handler::HttpResponse;

use super::CacheStoreContext;

pub(super) fn response_is_storable(context: &CacheStoreContext, response: &HttpResponse) -> bool {
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
    if !matches!(
        response.body().size_hint().upper(),
        Some(upper) if upper <= context.zone.config.max_entry_bytes as u64
    ) {
        return false;
    }
    true
}

fn response_is_grpc(headers: &HeaderMap) -> bool {
    headers.get(CONTENT_TYPE).and_then(|value| value.to_str().ok()).is_some_and(|content_type| {
        let mime = content_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
        mime.eq_ignore_ascii_case("application/grpc")
            || mime.starts_with("application/grpc+")
            || mime.starts_with("application/grpc-web")
    })
}

pub(super) fn response_ttl(headers: &HeaderMap, default_ttl: Duration) -> Duration {
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

fn expires_ttl(headers: &HeaderMap) -> Option<Duration> {
    let expires = headers.get(EXPIRES)?.to_str().ok()?;
    let expires = httpdate::parse_http_date(expires).ok()?;
    // An explicit stale Expires value means "do not cache"; default_ttl only applies
    // when upstream sends no freshness metadata.
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
