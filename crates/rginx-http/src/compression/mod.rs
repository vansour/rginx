use bytes::Bytes;
use http::header::{ACCEPT_RANGES, CONTENT_ENCODING, CONTENT_LENGTH, HeaderValue};
use http::{HeaderMap, Method, Response, StatusCode};
use http_body_util::BodyExt;
use hyper::body::Body as _;
use rginx_core::RouteBufferingPolicy;

use crate::handler::{HttpBody, HttpResponse, full_body};

mod accept_encoding;
mod content_type;
mod encode;
mod options;
#[cfg(test)]
mod tests;

use accept_encoding::{ContentCoding, merge_vary_header, preferred_response_encoding};
use content_type::response_is_eligible;
use encode::compress_bytes;
pub(crate) use options::ResponseCompressionOptions;

const MAX_COMPRESSIBLE_RESPONSE_BYTES: usize = 1024 * 1024;

pub async fn maybe_encode_response(
    method: &Method,
    request_headers: &HeaderMap,
    options: &ResponseCompressionOptions<'_>,
    response: HttpResponse,
) -> HttpResponse {
    if options.response_compression_disabled() {
        return response;
    }

    let Some(content_coding) = preferred_response_encoding(request_headers) else {
        return response;
    };

    if method == Method::HEAD {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    if !response_is_eligible(&parts.headers, parts.status, options) {
        return Response::from_parts(parts, body);
    }

    let Some(content_length) = compression_candidate_length(&parts.headers, &body, options) else {
        return Response::from_parts(parts, body);
    };

    if content_length > MAX_COMPRESSIBLE_RESPONSE_BYTES || content_length < options.min_bytes() {
        return Response::from_parts(parts, body);
    }

    let collected = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            tracing::warn!(
                %error,
                encoding = content_coding.label(),
                "failed to collect response body for compression"
            );
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(full_body(Bytes::new()))
                .expect("static 500 response should build");
        }
    };

    let compressed = match compress_bytes(content_coding, &collected) {
        Ok(compressed) if compressed.len() < collected.len() => compressed,
        Ok(_) => return buffered_response(parts, collected),
        Err(error) => {
            tracing::warn!(
                %error,
                encoding = content_coding.label(),
                "failed to compress response body"
            );
            return buffered_response(parts, collected);
        }
    };

    clear_compression_headers(&mut parts.headers);
    parts.headers.insert(CONTENT_ENCODING, HeaderValue::from_static(content_coding.header_value()));
    merge_vary_header(&mut parts.headers, "Accept-Encoding");
    set_content_length(&mut parts.headers, compressed.len());
    parts.headers.remove(ACCEPT_RANGES);

    Response::from_parts(parts, full_body(compressed))
}

fn compression_candidate_length(
    headers: &HeaderMap,
    body: &HttpBody,
    options: &ResponseCompressionOptions<'_>,
) -> Option<usize> {
    parse_content_length(headers).or_else(|| {
        (options.response_buffering == RouteBufferingPolicy::On)
            .then(|| body.size_hint().exact())
            .flatten()
            .and_then(|value| usize::try_from(value).ok())
    })
}

fn parse_content_length(headers: &HeaderMap) -> Option<usize> {
    headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
}

fn buffered_response(parts: http::response::Parts, body: Bytes) -> HttpResponse {
    let mut parts = parts;
    clear_compression_headers(&mut parts.headers);
    set_content_length(&mut parts.headers, body.len());
    Response::from_parts(parts, full_body(body))
}

fn clear_compression_headers(headers: &mut HeaderMap) {
    headers.remove(CONTENT_ENCODING);
    headers.remove(CONTENT_LENGTH);
}

fn set_content_length(headers: &mut HeaderMap, length: usize) {
    headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string())
            .expect("buffered body length should fit in a header"),
    );
}
