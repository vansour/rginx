use std::borrow::Cow;
use std::io::Write;

use brotli::CompressorWriter;
use bytes::Bytes;
use flate2::Compression;
use flate2::write::GzEncoder;
use http::header::{
    ACCEPT_ENCODING, ACCEPT_RANGES, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE,
    HeaderValue, VARY,
};
use http::{HeaderMap, Method, Response, StatusCode};
use http_body_util::BodyExt;
use hyper::body::Body as _;
use rginx_core::{Route, RouteBufferingPolicy, RouteCompressionPolicy};

use crate::handler::{HttpResponse, full_body};

const MIN_COMPRESSIBLE_RESPONSE_BYTES: usize = 256;
const MAX_COMPRESSIBLE_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResponseCompressionOptions<'a> {
    pub(crate) response_buffering: RouteBufferingPolicy,
    pub(crate) compression: RouteCompressionPolicy,
    pub(crate) compression_min_bytes: Option<usize>,
    pub(crate) compression_content_types: Cow<'a, [String]>,
}

impl Default for ResponseCompressionOptions<'_> {
    fn default() -> Self {
        Self {
            response_buffering: RouteBufferingPolicy::Auto,
            compression: RouteCompressionPolicy::Auto,
            compression_min_bytes: None,
            compression_content_types: Cow::Borrowed(&[]),
        }
    }
}

impl<'a> ResponseCompressionOptions<'a> {
    pub(crate) fn for_route(route: &'a Route) -> Self {
        Self {
            response_buffering: route.response_buffering,
            compression: route.compression,
            compression_min_bytes: route.compression_min_bytes,
            compression_content_types: Cow::Borrowed(route.compression_content_types.as_slice()),
        }
    }

    fn min_bytes(&self) -> usize {
        match self.compression {
            RouteCompressionPolicy::Force => self.compression_min_bytes.unwrap_or(1),
            RouteCompressionPolicy::Auto | RouteCompressionPolicy::Off => {
                self.compression_min_bytes.unwrap_or(MIN_COMPRESSIBLE_RESPONSE_BYTES)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContentCoding {
    Brotli,
    Gzip,
}

impl ContentCoding {
    fn header_value(self) -> &'static str {
        match self {
            Self::Brotli => "br",
            Self::Gzip => "gzip",
        }
    }

    fn label(self) -> &'static str {
        self.header_value()
    }
}

pub async fn maybe_encode_response(
    method: &Method,
    request_headers: &HeaderMap,
    options: &ResponseCompressionOptions<'_>,
    response: HttpResponse,
) -> HttpResponse {
    if options.compression == RouteCompressionPolicy::Off
        || options.response_buffering == RouteBufferingPolicy::Off
    {
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
            tracing::warn!(%error, encoding = content_coding.label(), "failed to collect response body for compression");
            let mut parts = parts_without_compression_metadata(parts);
            parts.status = StatusCode::INTERNAL_SERVER_ERROR;
            return Response::from_parts(parts, full_body(Bytes::new()));
        }
    };

    let compressed = match compress_bytes(content_coding, &collected) {
        Ok(compressed) if compressed.len() < collected.len() => compressed,
        Ok(_) => return Response::from_parts(parts, full_body(collected)),
        Err(error) => {
            tracing::warn!(%error, encoding = content_coding.label(), "failed to compress response body");
            return Response::from_parts(parts, full_body(collected));
        }
    };

    parts.headers.insert(CONTENT_ENCODING, HeaderValue::from_static(content_coding.header_value()));
    merge_vary_header(&mut parts.headers, "Accept-Encoding");
    parts.headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&compressed.len().to_string())
            .expect("compressed body length should fit in a header"),
    );
    parts.headers.remove(ACCEPT_RANGES);

    Response::from_parts(parts, full_body(compressed))
}

fn parts_without_compression_metadata(mut parts: http::response::Parts) -> http::response::Parts {
    parts.headers.remove(CONTENT_ENCODING);
    parts.headers.remove(CONTENT_LENGTH);
    parts.headers.remove(ACCEPT_RANGES);
    parts
}

fn response_is_eligible(
    headers: &HeaderMap,
    status: StatusCode,
    options: &ResponseCompressionOptions<'_>,
) -> bool {
    if status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED
    {
        return false;
    }

    if status == StatusCode::PARTIAL_CONTENT
        || headers.contains_key(CONTENT_RANGE)
        || headers.contains_key(CONTENT_ENCODING)
    {
        return false;
    }

    let Some(content_type) = headers.get(CONTENT_TYPE).and_then(|value| value.to_str().ok()) else {
        return false;
    };

    content_type_is_eligible(content_type, options)
}

fn compression_candidate_length(
    headers: &HeaderMap,
    body: &crate::handler::HttpBody,
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

fn merge_vary_header(headers: &mut HeaderMap, token: &str) {
    let mut values = Vec::<String>::new();

    for value in headers.get_all(VARY).iter() {
        let Ok(value) = value.to_str() else {
            continue;
        };
        for item in value.split(',').map(str::trim).filter(|item| !item.is_empty()) {
            if item == "*" {
                headers.insert(VARY, HeaderValue::from_static("*"));
                return;
            }
            if !values.iter().any(|existing| existing.eq_ignore_ascii_case(item)) {
                values.push(item.to_string());
            }
        }
    }

    if !values.iter().any(|existing| existing.eq_ignore_ascii_case(token)) {
        values.push(token.to_string());
    }

    if let Ok(value) = HeaderValue::from_str(&values.join(", ")) {
        headers.insert(VARY, value);
    } else {
        headers.append(VARY, HeaderValue::from_static("Accept-Encoding"));
    }
}

fn preferred_response_encoding(headers: &HeaderMap) -> Option<ContentCoding> {
    #[derive(Default)]
    struct AcceptedEncodings {
        brotli: Option<f32>,
        gzip: Option<f32>,
        wildcard: Option<f32>,
    }

    impl AcceptedEncodings {
        fn record(&mut self, coding: &str, q: f32) {
            let slot = if coding.eq_ignore_ascii_case("br") {
                Some(&mut self.brotli)
            } else if coding.eq_ignore_ascii_case("gzip") {
                Some(&mut self.gzip)
            } else if coding == "*" {
                Some(&mut self.wildcard)
            } else {
                None
            };

            if let Some(slot) = slot {
                let updated = (*slot).map_or(q, |current| current.max(q));
                *slot = Some(updated);
            }
        }

        fn quality_for(&self, coding: ContentCoding) -> f32 {
            match coding {
                ContentCoding::Brotli => self.brotli.or(self.wildcard).unwrap_or(0.0),
                ContentCoding::Gzip => self.gzip.or(self.wildcard).unwrap_or(0.0),
            }
        }
    }

    let mut accepted = AcceptedEncodings::default();
    for (coding, q) in headers
        .get_all(ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(parse_accept_encoding_item)
    {
        accepted.record(coding, q);
    }

    let brotli_q = accepted.quality_for(ContentCoding::Brotli);
    let gzip_q = accepted.quality_for(ContentCoding::Gzip);

    match (brotli_q > 0.0, gzip_q > 0.0) {
        (false, false) => None,
        (true, false) => Some(ContentCoding::Brotli),
        (false, true) => Some(ContentCoding::Gzip),
        (true, true) if brotli_q >= gzip_q => Some(ContentCoding::Brotli),
        (true, true) => Some(ContentCoding::Gzip),
    }
}

fn parse_accept_encoding_item(item: &str) -> Option<(&str, f32)> {
    let mut parts = item.split(';');
    let coding = parts.next()?.trim();
    if coding.is_empty() {
        return None;
    }

    let mut q = 1.0;
    for part in parts {
        let part = part.trim();
        if let Some(value) = part.strip_prefix("q=") {
            q = value.parse::<f32>().ok()?;
        }
    }

    Some((coding, q))
}

fn is_compressible_content_type(content_type: &str) -> bool {
    let mime = content_type.split(';').next().unwrap_or(content_type).trim();

    mime.starts_with("text/")
        || matches!(
            mime,
            "application/json"
                | "application/problem+json"
                | "application/javascript"
                | "application/xml"
                | "application/xhtml+xml"
                | "image/svg+xml"
        )
}

fn content_type_is_eligible(content_type: &str, options: &ResponseCompressionOptions<'_>) -> bool {
    let mime = content_type.split(';').next().unwrap_or(content_type).trim();
    if options.compression_content_types.is_empty() {
        return is_compressible_content_type(mime);
    }

    options
        .compression_content_types
        .iter()
        .any(|candidate| candidate.trim().eq_ignore_ascii_case(mime))
}

fn compress_bytes(coding: ContentCoding, bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    match coding {
        ContentCoding::Brotli => brotli_bytes(bytes),
        ContentCoding::Gzip => gzip_bytes(bytes),
    }
}

fn brotli_bytes(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut compressed = Vec::with_capacity(bytes.len() / 2);
    {
        let mut encoder = CompressorWriter::new(&mut compressed, 4096, 5, 22);
        encoder.write_all(bytes)?;
        encoder.flush()?;
    }
    Ok(compressed)
}

fn gzip_bytes(bytes: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::with_capacity(bytes.len() / 2), Compression::default());
    encoder.write_all(bytes)?;
    encoder.finish()
}

#[cfg(test)]
mod tests;
