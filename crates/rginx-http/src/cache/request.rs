use http::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, RANGE};
use http::{Method, Uri};
use rginx_core::{CacheKeyRenderContext, CachePredicateRequestContext, RouteCachePolicy};

use super::CacheRequest;
use super::vary::normalized_accept_encoding;

pub(super) fn cache_request_bypass(request: &CacheRequest, policy: &RouteCachePolicy) -> bool {
    if !policy.methods.contains(&request.method) {
        return true;
    }

    if !matches!(request.method, Method::GET | Method::HEAD) {
        return true;
    }

    if request.headers.contains_key(AUTHORIZATION) || request.headers.contains_key(RANGE) {
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
    rendered
}
