use http::HeaderMap;
use http::header::CONTENT_TYPE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::handler) struct GrpcRequestMetadata<'a> {
    pub(crate) protocol: &'static str,
    pub(crate) service: &'a str,
    pub(crate) method: &'a str,
}

pub(in crate::handler) fn grpc_request_metadata<'a>(
    headers: &HeaderMap,
    request_path: &'a str,
) -> Option<GrpcRequestMetadata<'a>> {
    let protocol = grpc_protocol(headers)?;
    let (service, method) = grpc_service_method(request_path)?;

    Some(GrpcRequestMetadata { protocol, service, method })
}

pub(super) fn grpc_protocol(headers: &HeaderMap) -> Option<&'static str> {
    let (mime, _) =
        split_header_content_type(crate::handler::dispatch::header_value(headers, CONTENT_TYPE)?);
    let mime = mime.to_ascii_lowercase();
    if mime.starts_with("application/grpc-web-text") {
        Some("grpc-web-text")
    } else if mime.starts_with("application/grpc-web") {
        Some("grpc-web")
    } else if mime.starts_with("application/grpc") {
        Some("grpc")
    } else {
        None
    }
}

fn grpc_service_method(path: &str) -> Option<(&str, &str)> {
    let path = path.strip_prefix('/')?;
    let (service, method) = path.split_once('/')?;
    if service.is_empty() || method.is_empty() || method.contains('/') {
        return None;
    }
    Some((service, method))
}

fn split_header_content_type(content_type: &str) -> (&str, &str) {
    let mut parts = content_type.splitn(2, ';');
    let mime = parts.next().unwrap_or_default().trim();
    let params = parts.next().unwrap_or_default().trim();
    (mime, params)
}
