use http::header::HOST;
use http::{HeaderMap, Uri};
use rginx_core::{ConfigSnapshot, VirtualHost};

use crate::router;

use super::super::grpc::GrpcRequestMetadata;

pub(in crate::handler) fn request_host<'a>(headers: &'a HeaderMap, uri: &'a Uri) -> &'a str {
    headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| uri.authority().map(|authority| authority.as_str()))
        .unwrap_or_default()
}

pub(super) fn select_vhost_for_request<'a>(
    config: &'a ConfigSnapshot,
    headers: &HeaderMap,
    uri: &Uri,
) -> &'a VirtualHost {
    let host = request_host(headers, uri).to_string();
    router::select_vhost(&config.vhosts, &config.default_vhost, &host)
}

pub(super) fn route_match_context<'a>(
    request_path: &'a str,
    grpc_request: Option<GrpcRequestMetadata<'a>>,
) -> router::RouteMatchContext<'a> {
    let grpc = grpc_request.map(|metadata| router::GrpcRequestMatch {
        service: metadata.service,
        method: metadata.method,
    });

    router::RouteMatchContext { path: request_path, grpc }
}

#[cfg(test)]
pub(in crate::handler) fn select_route_for_request<'a>(
    config: &'a ConfigSnapshot,
    headers: &HeaderMap,
    uri: &Uri,
) -> Option<&'a rginx_core::Route> {
    use super::super::grpc::grpc_request_metadata;
    use crate::request_target::normalize_request_target;

    let vhost = select_vhost_for_request(config, headers, uri);
    let normalized = normalize_request_target(uri);
    let context =
        route_match_context(&normalized.path, grpc_request_metadata(headers, &normalized.path));
    router::select_route_in_vhost_with_context(vhost, &context)
}
