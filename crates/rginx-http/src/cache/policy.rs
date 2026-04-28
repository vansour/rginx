use std::time::{Duration, SystemTime};

use http::StatusCode;
use http::header::{
    CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, EXPIRES, HeaderMap, HeaderName,
    SET_COOKIE, VARY,
};
use hyper::body::Body as _;

use crate::handler::HttpResponse;

use super::CacheStoreContext;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ResponseFreshness {
    pub(super) ttl: Duration,
    pub(super) stale_if_error: Option<Duration>,
    pub(super) stale_while_revalidate: Option<Duration>,
    pub(super) must_revalidate: bool,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ResponseBodySize {
    exact: Option<u64>,
    upper: Option<u64>,
}

pub(super) fn response_is_storable(context: &CacheStoreContext, response: &HttpResponse) -> bool {
    response_is_storable_with_size(
        context,
        response.status(),
        response.headers(),
        ResponseBodySize::from_response(response),
    )
}

pub(super) fn response_is_storable_with_size(
    context: &CacheStoreContext,
    status: StatusCode,
    headers: &HeaderMap,
    body_size: ResponseBodySize,
) -> bool {
    if !context.policy.statuses.contains(&status) {
        return false;
    }
    if status == StatusCode::PARTIAL_CONTENT
        || headers.contains_key(CONTENT_RANGE)
        || headers.contains_key(SET_COOKIE)
    {
        return false;
    }
    if !vary_is_supported(headers) {
        return false;
    }
    if response_is_grpc(headers) {
        return false;
    }
    if cache_control_contains(headers, &["no-store", "private"]) {
        return false;
    }
    if let Some(length) = parse_content_length(headers)
        && length > context.zone.config.max_entry_bytes
    {
        return false;
    }
    if let Some(exact) = body_size.exact
        && exact > context.zone.config.max_entry_bytes as u64
    {
        return false;
    }
    if !matches!(body_size.upper, Some(upper) if upper <= context.zone.config.max_entry_bytes as u64)
    {
        return false;
    }
    true
}

pub(super) fn response_freshness(
    context: &CacheStoreContext,
    headers: &HeaderMap,
) -> ResponseFreshness {
    ResponseFreshness {
        ttl: response_ttl(headers, context.zone.config.default_ttl),
        stale_if_error: cache_control_duration(headers, "stale-if-error")
            .or(context.policy.stale_if_error),
        stale_while_revalidate: cache_control_duration(headers, "stale-while-revalidate"),
        must_revalidate: cache_control_contains(headers, &["no-cache", "must-revalidate"]),
    }
}

pub(super) fn request_requires_revalidation(headers: &HeaderMap) -> bool {
    cache_control_contains(headers, &["no-cache"])
        || cache_control_duration(headers, "max-age") == Some(Duration::ZERO)
}

pub(super) fn header_value(headers: &HeaderMap, name: HeaderName) -> Option<String> {
    headers.get(name).and_then(|value| value.to_str().ok()).map(str::to_string)
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

pub(super) fn cache_control_duration(
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

pub(super) fn vary_is_supported(headers: &HeaderMap) -> bool {
    if headers.get(VARY).is_none() {
        return true;
    }

    headers.get_all(VARY).iter().all(|value| {
        value.to_str().ok().is_some_and(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .all(|token| !token.eq("*") && token.eq_ignore_ascii_case("accept-encoding"))
        })
    })
}

fn parse_content_length(headers: &HeaderMap) -> Option<usize> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
}

impl ResponseBodySize {
    fn from_response(response: &HttpResponse) -> Self {
        Self {
            exact: response.body().size_hint().exact(),
            upper: response.body().size_hint().upper(),
        }
    }

    pub(super) fn exact(body_size_bytes: usize) -> Self {
        let body_size_bytes = body_size_bytes as u64;
        Self { exact: Some(body_size_bytes), upper: Some(body_size_bytes) }
    }
}
