use http::header::{AUTHORIZATION, CONTENT_RANGE, CONTENT_TYPE, HeaderMap, RANGE};
use http::{Method, Uri};
use rginx_core::{
    CacheKeyRenderContext, CachePredicateRequestContext, CacheRangeRequestPolicy, RouteCachePolicy,
};

use super::CacheRequest;
use super::vary::normalized_accept_encoding;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CacheableRangeRequest {
    pub(super) start: u64,
    pub(super) end: u64,
}

pub(super) fn cache_request_bypass(request: &CacheRequest, policy: &RouteCachePolicy) -> bool {
    if !policy.methods.contains(&request.method) {
        return true;
    }

    if !matches!(request.method, Method::GET | Method::HEAD) {
        return true;
    }

    if request.headers.contains_key(AUTHORIZATION) {
        return true;
    }

    if request.headers.contains_key(RANGE) && cacheable_range_request(request, policy).is_none() {
        return true;
    }

    request.headers.get(CONTENT_TYPE).and_then(|value| value.to_str().ok()).is_some_and(
        |content_type| {
            let mime =
                content_type.split(';').next().unwrap_or_default().trim().to_ascii_lowercase();
            mime == "application/grpc"
                || mime.starts_with("application/grpc+")
                || mime.starts_with("application/grpc-web")
        },
    ) || policy.cache_bypass.as_ref().is_some_and(|predicate| {
        predicate.matches_request(&CachePredicateRequestContext {
            method: &request.method,
            uri: request.request_uri(),
            headers: &request.headers,
        })
    })
}

pub(super) fn render_cache_key(
    method: &Method,
    uri: &Uri,
    headers: &HeaderMap,
    scheme: &str,
    policy: &RouteCachePolicy,
) -> String {
    let request_uri = uri.path_and_query().map(|value| value.as_str()).unwrap_or("/");
    let host = headers
        .get(http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .or_else(|| uri.authority().map(|authority| authority.as_str()))
        .unwrap_or("-");
    let mut rendered = policy.key.render(&CacheKeyRenderContext {
        scheme,
        host,
        uri: request_uri,
        method: method.as_str(),
        headers,
    });
    if let Some(accept_encoding) = normalized_accept_encoding(headers) {
        rendered.push_str("|ae:");
        rendered.push_str(&accept_encoding);
    }
    if let Some(range) = cacheable_range_request_from_parts(method, headers, policy) {
        rendered.push_str("|range:");
        rendered.push_str(&range.start.to_string());
        rendered.push('-');
        rendered.push_str(&range.end.to_string());
    }
    rendered
}

pub(super) fn cacheable_range_request(
    request: &CacheRequest,
    policy: &RouteCachePolicy,
) -> Option<CacheableRangeRequest> {
    cacheable_range_request_from_parts(&request.method, &request.headers, policy)
}

pub(super) fn response_content_range_matches_request(
    request: &CacheRequest,
    policy: &RouteCachePolicy,
    headers: &HeaderMap,
) -> bool {
    let Some(expected) = cacheable_range_request(request, policy) else {
        return false;
    };
    let Some(value) = headers.get(CONTENT_RANGE) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let Some(value) = value.trim().strip_prefix("bytes ") else {
        return false;
    };
    let Some((range, _total)) = value.split_once('/') else {
        return false;
    };
    let Some((start, end)) = range.split_once('-') else {
        return false;
    };
    let Ok(start) = start.trim().parse::<u64>() else {
        return false;
    };
    let Ok(end) = end.trim().parse::<u64>() else {
        return false;
    };
    start == expected.start && end == expected.end
}

fn cacheable_range_request_from_parts(
    method: &Method,
    headers: &HeaderMap,
    policy: &RouteCachePolicy,
) -> Option<CacheableRangeRequest> {
    if policy.range_requests != CacheRangeRequestPolicy::Cache
        || !matches!(*method, Method::GET | Method::HEAD)
    {
        return None;
    }
    let value = headers.get(RANGE)?.to_str().ok()?;
    parse_single_bounded_byte_range(value)
}

fn parse_single_bounded_byte_range(value: &str) -> Option<CacheableRangeRequest> {
    let value = value.trim().strip_prefix("bytes=")?.trim();
    if value.contains(',') {
        return None;
    }
    let (start, end) = value.split_once('-')?;
    if start.trim().is_empty() || end.trim().is_empty() {
        return None;
    }
    let start = start.trim().parse::<u64>().ok()?;
    let end = end.trim().parse::<u64>().ok()?;
    (start <= end).then_some(CacheableRangeRequest { start, end })
}
