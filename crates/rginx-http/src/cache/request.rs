use http::HeaderValue;
use http::header::{AUTHORIZATION, CONTENT_RANGE, CONTENT_TYPE, HeaderMap, IF_RANGE, RANGE};
use http::{Method, Uri};
use rginx_core::{
    CacheKeyRenderContext, CachePredicateRequestContext, CacheRangeRequestPolicy, RouteCachePolicy,
};

use super::CacheRequest;
use super::vary::normalized_accept_encoding;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CacheableRangeRequest {
    pub(super) request_start: u64,
    pub(super) request_end: u64,
    pub(super) cache_start: u64,
    pub(super) cache_end: u64,
}

pub(super) fn cache_request_bypass(request: &CacheRequest, policy: &RouteCachePolicy) -> bool {
    let effective_method = cache_key_method(&request.method, policy);
    if !policy.methods.contains(effective_method) {
        return true;
    }

    if !matches!(request.method, Method::GET | Method::HEAD) {
        return true;
    }

    if request.headers.contains_key(AUTHORIZATION) {
        return true;
    }

    if request_cache_control_contains(&request.headers, &["no-store"]) {
        return true;
    }

    if request.headers.contains_key(IF_RANGE) {
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
    let cache_method = cache_key_method(method, policy);
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
        method: cache_method.as_str(),
        headers,
    });
    if !policy.convert_head && !policy.key.references_method() {
        rendered.push_str("|cache-method:");
        rendered.push_str(cache_method.as_str());
    }
    if let Some(accept_encoding) = normalized_accept_encoding(headers) {
        rendered.push_str("|ae:");
        rendered.push_str(&accept_encoding);
    }
    if let Some(range) = cacheable_range_request_from_parts(cache_method, headers, policy) {
        rendered.push_str("|range:");
        rendered.push_str(&range.cache_start.to_string());
        rendered.push('-');
        rendered.push_str(&range.cache_end.to_string());
    }
    rendered
}

pub(super) fn cacheable_range_request(
    request: &CacheRequest,
    policy: &RouteCachePolicy,
) -> Option<CacheableRangeRequest> {
    cacheable_range_request_from_parts(
        cache_key_method(&request.method, policy),
        &request.headers,
        policy,
    )
}

pub(super) fn upstream_cache_request_method(method: &Method, policy: &RouteCachePolicy) -> Method {
    cache_key_method(method, policy).clone()
}

pub(super) fn apply_upstream_range_headers(
    method: &Method,
    headers: &mut HeaderMap,
    policy: &RouteCachePolicy,
) {
    let Some(range) =
        cacheable_range_request_from_parts(cache_key_method(method, policy), headers, policy)
    else {
        return;
    };
    headers.remove(RANGE);
    headers.insert(
        RANGE,
        HeaderValue::from_str(&range.upstream_header_value())
            .expect("normalized cache range header should be valid"),
    );
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
    start == expected.cache_start && end >= expected.request_end && end <= expected.cache_end
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
    let mut values = headers.get_all(RANGE).iter();
    let value = values.next()?.to_str().ok()?;
    if values.next().is_some() {
        return None;
    }
    cacheable_range_request_for_policy(parse_single_bounded_byte_range(value)?, policy)
}

fn cache_key_method<'a>(method: &'a Method, policy: &RouteCachePolicy) -> &'a Method {
    if *method == Method::HEAD && policy.convert_head { &Method::GET } else { method }
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
    (start <= end).then_some(CacheableRangeRequest {
        request_start: start,
        request_end: end,
        cache_start: start,
        cache_end: end,
    })
}

fn cacheable_range_request_for_policy(
    request: CacheableRangeRequest,
    policy: &RouteCachePolicy,
) -> Option<CacheableRangeRequest> {
    let Some(slice_size) = policy.slice_size_bytes else {
        return Some(request);
    };
    let slice_start = request.request_start / slice_size * slice_size;
    let slice_end = slice_start.saturating_add(slice_size.saturating_sub(1));
    (request.request_end <= slice_end).then_some(CacheableRangeRequest {
        cache_start: slice_start,
        cache_end: slice_end,
        ..request
    })
}

impl CacheableRangeRequest {
    pub(super) fn upstream_header_value(self) -> String {
        format!("bytes={}-{}", self.cache_start, self.cache_end)
    }

    pub(super) fn needs_downstream_trimming(self) -> bool {
        self.request_start != self.cache_start || self.request_end != self.cache_end
    }
}

fn request_cache_control_contains(headers: &HeaderMap, directives: &[&str]) -> bool {
    headers.get_all(http::header::CACHE_CONTROL).iter().any(|value| {
        value.to_str().ok().is_some_and(|value| {
            value.split(',').any(|directive| {
                let name = directive.split_once('=').map_or(directive, |(name, _)| name).trim();
                directives.iter().any(|expected| name.eq_ignore_ascii_case(expected))
            })
        })
    })
}
