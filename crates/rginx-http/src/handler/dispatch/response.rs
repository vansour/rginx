use http::header::CONTENT_LENGTH;
use http::{HeaderMap, HeaderValue, Method, Response, StatusCode, Version};

use crate::compression::ResponseCompressionOptions;
use crate::handler::HttpResponse;
use crate::handler::grpc::{GrpcObservability, GrpcRequestMetadata, grpc_observability};

use super::date::current_http_date;
use super::full_body;

pub(in crate::handler) struct FinalizedDownstreamResponse {
    pub(in crate::handler) response: HttpResponse,
    pub(in crate::handler) status: StatusCode,
    pub(in crate::handler) body_bytes_sent: Option<u64>,
    pub(in crate::handler) grpc: Option<GrpcObservability>,
}

pub(in crate::handler) async fn finalize_downstream_response(
    method: &Method,
    request_headers: &HeaderMap,
    response_compression_options: &ResponseCompressionOptions<'_>,
    request_id_header: HeaderValue,
    mut response: HttpResponse,
    grpc_request: Option<GrpcRequestMetadata<'_>>,
    alt_svc_header: Option<HeaderValue>,
    server_header: HeaderValue,
) -> FinalizedDownstreamResponse {
    let grpc = grpc_observability(grpc_request, response.headers());
    if grpc.is_none() && *method != Method::HEAD {
        response = crate::compression::maybe_encode_response(
            method,
            request_headers,
            response_compression_options,
            response,
        )
        .await;
    }
    if *method == Method::HEAD {
        response = strip_response_body(response);
    }
    if let Some(alt_svc_header) = alt_svc_header {
        response.headers_mut().insert(http::header::ALT_SVC, alt_svc_header);
    }
    if !response.headers().contains_key(http::header::DATE) {
        response.headers_mut().insert(http::header::DATE, current_http_date());
    }
    response.headers_mut().insert(http::header::SERVER, server_header);
    response.headers_mut().insert("x-request-id", request_id_header);

    let status = response.status();
    let body_bytes_sent = response_body_bytes_sent(method.as_str(), &response);
    FinalizedDownstreamResponse { response, status, body_bytes_sent, grpc }
}

pub(in crate::handler) fn response_body_bytes_sent(
    method: &str,
    response: &HttpResponse,
) -> Option<u64> {
    if method == Method::HEAD.as_str() {
        return Some(0);
    }

    response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
}

pub(in crate::handler) fn http_version_label(version: Version) -> &'static str {
    match version {
        Version::HTTP_09 => "HTTP/0.9",
        Version::HTTP_10 => "HTTP/1.0",
        Version::HTTP_11 => "HTTP/1.1",
        Version::HTTP_2 => "HTTP/2.0",
        Version::HTTP_3 => "HTTP/3.0",
        _ => "HTTP/?",
    }
}

pub(in crate::handler) fn header_value(
    headers: &HeaderMap,
    name: http::header::HeaderName,
) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok()).map(str::trim)
}

fn strip_response_body(response: HttpResponse) -> HttpResponse {
    let (parts, _body) = response.into_parts();
    Response::from_parts(parts, full_body(bytes::Bytes::new()))
}

pub(super) fn alt_svc_header_value(
    listener: &rginx_core::Listener,
    request_version: Version,
) -> Option<HeaderValue> {
    if request_version == Version::HTTP_3 || !listener.tls_enabled() {
        return None;
    }

    let http3 = listener.http3.as_ref()?;
    if !http3.advertise_alt_svc {
        return None;
    }

    let value =
        format!("h3=\":{}\"; ma={}", http3.listen_addr.port(), http3.alt_svc_max_age.as_secs());
    HeaderValue::from_str(&value).ok()
}
