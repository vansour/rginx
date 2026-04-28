use std::time::Duration;

use http::StatusCode;
use http::header::{
    CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, HeaderMap, HeaderName, SET_COOKIE, VARY,
};
use hyper::body::Body as _;
use rginx_core::CacheIgnoreHeader;
use rginx_core::CachePredicateRequestContext;

use crate::handler::HttpResponse;

use super::CacheStoreContext;
use super::request::{cacheable_range_request, response_content_range_matches_request};

mod directives;

use directives::{
    cache_control_contains, cache_control_duration, cache_control_max_age, expires_ttl,
    pragma_contains, x_accel_expires_ttl,
};

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
    let requested_range = cacheable_range_request(&context.request, &context.policy);
    if !status_is_cacheable(context, status) {
        return false;
    }
    if requested_range.is_some() && status != StatusCode::PARTIAL_CONTENT {
        return false;
    }
    if status == StatusCode::PARTIAL_CONTENT {
        if requested_range.is_none()
            || !response_content_range_matches_request(&context.request, &context.policy, headers)
        {
            return false;
        }
    } else if headers.contains_key(CONTENT_RANGE) {
        return false;
    }
    if !ignores_header(context, CacheIgnoreHeader::SetCookie) && headers.contains_key(SET_COOKIE) {
        return false;
    }
    if !vary_is_supported(context, headers) {
        return false;
    }
    if response_is_grpc(headers) {
        return false;
    }
    if !ignores_header(context, CacheIgnoreHeader::CacheControl)
        && cache_control_contains(headers, &["no-store", "private"])
    {
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
    status: StatusCode,
    headers: &HeaderMap,
) -> ResponseFreshness {
    ResponseFreshness {
        ttl: response_ttl(context, status, headers),
        stale_if_error: (!ignores_header(context, CacheIgnoreHeader::CacheControl))
            .then(|| cache_control_duration(headers, "stale-if-error"))
            .flatten()
            .or(context.policy.stale_if_error),
        stale_while_revalidate: (!ignores_header(context, CacheIgnoreHeader::CacheControl))
            .then(|| cache_control_duration(headers, "stale-while-revalidate"))
            .flatten(),
        must_revalidate: !ignores_header(context, CacheIgnoreHeader::CacheControl)
            && cache_control_contains(headers, &["no-cache", "must-revalidate"]),
    }
}

pub(super) fn request_requires_revalidation(headers: &HeaderMap) -> bool {
    cache_control_contains(headers, &["no-cache"])
        || cache_control_duration(headers, "max-age") == Some(Duration::ZERO)
        || pragma_contains(headers, "no-cache")
}

pub(super) fn header_value(headers: &HeaderMap, name: HeaderName) -> Option<String> {
    headers.get(name).and_then(|value| value.to_str().ok()).map(str::to_string)
}

pub(super) fn response_no_cache(context: &CacheStoreContext, status: StatusCode) -> bool {
    context.policy.no_cache.as_ref().is_some_and(|predicate| {
        predicate.matches_response(
            &CachePredicateRequestContext {
                method: &context.request.method,
                uri: context.request.request_uri(),
                headers: &context.request.headers,
            },
            status,
        )
    })
}

fn response_is_grpc(headers: &HeaderMap) -> bool {
    headers.get(CONTENT_TYPE).and_then(|value| value.to_str().ok()).is_some_and(|content_type| {
        let mime = content_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
        mime.eq_ignore_ascii_case("application/grpc")
            || mime.starts_with("application/grpc+")
            || mime.starts_with("application/grpc-web")
    })
}

pub(super) fn response_ttl(
    context: &CacheStoreContext,
    status: StatusCode,
    headers: &HeaderMap,
) -> Duration {
    (!ignores_header(context, CacheIgnoreHeader::XAccelExpires))
        .then(|| x_accel_expires_ttl(headers))
        .flatten()
        .or_else(|| {
            (!ignores_header(context, CacheIgnoreHeader::CacheControl))
                .then(|| cache_control_max_age(headers))
                .flatten()
        })
        .or_else(|| {
            (!ignores_header(context, CacheIgnoreHeader::Expires))
                .then(|| expires_ttl(headers))
                .flatten()
        })
        .or_else(|| {
            context
                .policy
                .ttl_by_status
                .iter()
                .find_map(|rule| rule.statuses.contains(&status).then_some(rule.ttl))
        })
        .unwrap_or(context.zone.config.default_ttl)
}

pub(super) fn vary_is_supported(context: &CacheStoreContext, headers: &HeaderMap) -> bool {
    ignores_header(context, CacheIgnoreHeader::Vary) || vary_headers(headers).is_some()
}

pub(super) fn vary_headers(headers: &HeaderMap) -> Option<Vec<HeaderName>> {
    let mut names = Vec::new();
    for value in headers.get_all(VARY) {
        let value = value.to_str().ok()?;
        for token in value.split(',').map(str::trim).filter(|token| !token.is_empty()) {
            if token == "*" {
                return None;
            }
            let name = token.parse::<HeaderName>().ok()?;
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    Some(names)
}

fn status_is_cacheable(context: &CacheStoreContext, status: StatusCode) -> bool {
    context.policy.statuses.contains(&status)
        || (status == StatusCode::PARTIAL_CONTENT
            && cacheable_range_request(&context.request, &context.policy).is_some())
}

fn ignores_header(context: &CacheStoreContext, header: CacheIgnoreHeader) -> bool {
    context.policy.ignore_headers.contains(&header)
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
