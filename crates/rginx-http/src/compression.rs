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

use crate::handler::{HttpResponse, full_body};

const MIN_COMPRESSIBLE_RESPONSE_BYTES: usize = 256;
const MAX_COMPRESSIBLE_RESPONSE_BYTES: usize = 1024 * 1024;

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
    response: HttpResponse,
) -> HttpResponse {
    let Some(content_coding) = preferred_response_encoding(request_headers) else {
        return response;
    };

    if method == Method::HEAD {
        return response;
    }

    let (mut parts, body) = response.into_parts();
    if !response_is_eligible(&parts.headers, parts.status) {
        return Response::from_parts(parts, body);
    }

    let Some(content_length) = parse_content_length(&parts.headers) else {
        return Response::from_parts(parts, body);
    };

    if !(MIN_COMPRESSIBLE_RESPONSE_BYTES..=MAX_COMPRESSIBLE_RESPONSE_BYTES)
        .contains(&content_length)
    {
        return Response::from_parts(parts, body);
    }

    let collected = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(error) => {
            tracing::warn!(%error, encoding = content_coding.label(), "failed to collect response body for compression");
            return Response::from_parts(
                parts_without_compression_metadata(parts),
                full_body(Bytes::new()),
            );
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

fn response_is_eligible(headers: &HeaderMap, status: StatusCode) -> bool {
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

    is_compressible_content_type(content_type)
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
mod tests {
    use std::pin::Pin;
    use std::task::{Context, Poll};

    use brotli::Decompressor;
    use bytes::Bytes;
    use flate2::read::GzDecoder;
    use http::header::{
        ACCEPT_ENCODING, ACCEPT_RANGES, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, VARY,
    };
    use http::{HeaderMap, HeaderValue, Method, Response, StatusCode};
    use http_body_util::BodyExt;
    use hyper::body::{Frame, SizeHint};
    use std::io::Read;

    use super::{ContentCoding, maybe_encode_response, preferred_response_encoding};
    use crate::handler::{BoxError, text_response};

    #[derive(Debug, Default)]
    struct CollectErrorBody;

    impl hyper::body::Body for CollectErrorBody {
        type Data = Bytes;
        type Error = std::io::Error;

        fn poll_frame(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
            Poll::Ready(Some(Err(std::io::Error::other("collect failed"))))
        }

        fn is_end_stream(&self) -> bool {
            false
        }

        fn size_hint(&self) -> SizeHint {
            let mut hint = SizeHint::new();
            hint.set_exact(512);
            hint
        }
    }

    #[test]
    fn preferred_response_encoding_honors_quality_values() {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("br, gzip;q=0.5"));
        assert_eq!(preferred_response_encoding(&headers), Some(ContentCoding::Brotli));

        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("br;q=0.1, gzip;q=0.9"));
        assert_eq!(preferred_response_encoding(&headers), Some(ContentCoding::Gzip));

        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("br;q=0, *;q=0.5"));
        assert_eq!(preferred_response_encoding(&headers), Some(ContentCoding::Gzip));

        headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip;q=0"));
        assert_eq!(preferred_response_encoding(&headers), None);
    }

    #[tokio::test]
    async fn maybe_encode_response_brotlis_text_bodies() {
        let mut request_headers = HeaderMap::new();
        request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("br, gzip;q=0.5"));

        let response = text_response(
            StatusCode::OK,
            "text/plain; charset=utf-8",
            "hello brotli world\n".repeat(32),
        );

        let response = maybe_encode_response(&Method::GET, &request_headers, response).await;
        assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "br");
        assert_eq!(response.headers().get(http::header::VARY).unwrap(), "Accept-Encoding");

        let compressed = response
            .into_body()
            .collect()
            .await
            .expect("compressed body should collect")
            .to_bytes();
        let mut decoder = Decompressor::new(compressed.as_ref(), 4096);
        let mut decoded = String::new();
        decoder.read_to_string(&mut decoded).expect("brotli payload should decode");

        assert_eq!(decoded, "hello brotli world\n".repeat(32));
    }

    #[tokio::test]
    async fn maybe_encode_response_gzips_text_bodies() {
        let mut request_headers = HeaderMap::new();
        request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip, br;q=0.1"));

        let response = text_response(
            StatusCode::OK,
            "text/plain; charset=utf-8",
            "hello gzip world\n".repeat(32),
        );

        let response = maybe_encode_response(&Method::GET, &request_headers, response).await;
        assert_eq!(response.headers().get(CONTENT_ENCODING).unwrap(), "gzip");
        assert_eq!(response.headers().get(http::header::VARY).unwrap(), "Accept-Encoding");

        let compressed = response
            .into_body()
            .collect()
            .await
            .expect("compressed body should collect")
            .to_bytes();
        let mut decoder = GzDecoder::new(compressed.as_ref());
        let mut decoded = String::new();
        decoder.read_to_string(&mut decoded).expect("gzip payload should decode");

        assert_eq!(decoded, "hello gzip world\n".repeat(32));
    }

    #[tokio::test]
    async fn maybe_encode_response_skips_partial_content() {
        let mut request_headers = HeaderMap::new();
        request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

        let response = Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(CONTENT_TYPE, "text/plain; charset=utf-8")
            .header(http::header::CONTENT_LENGTH, "512")
            .header(http::header::CONTENT_RANGE, "bytes 0-511/2048")
            .header(ACCEPT_RANGES, "bytes")
            .body(crate::handler::full_body(Bytes::from(vec![b'a'; 512])))
            .expect("partial content response should build");

        let response = maybe_encode_response(&Method::GET, &request_headers, response).await;
        assert!(response.headers().get(CONTENT_ENCODING).is_none());
        assert_eq!(
            response.headers().get(http::header::CONTENT_RANGE).unwrap(),
            "bytes 0-511/2048"
        );
    }

    #[tokio::test]
    async fn maybe_encode_response_skips_small_or_binary_bodies() {
        let mut request_headers = HeaderMap::new();
        request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

        let small = text_response(StatusCode::OK, "text/plain; charset=utf-8", "tiny");
        let small = maybe_encode_response(&Method::GET, &request_headers, small).await;
        assert!(small.headers().get(CONTENT_ENCODING).is_none());

        let binary = Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "application/grpc")
            .header(http::header::CONTENT_LENGTH, "512")
            .body(crate::handler::full_body(Bytes::from(vec![0_u8; 512])))
            .expect("binary response should build");
        let binary = maybe_encode_response(&Method::GET, &request_headers, binary).await;
        assert!(binary.headers().get(CONTENT_ENCODING).is_none());
    }

    #[tokio::test]
    async fn maybe_encode_response_merges_existing_vary_header() {
        let mut request_headers = HeaderMap::new();
        request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

        let response = Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/plain; charset=utf-8")
            .header(CONTENT_LENGTH, "640")
            .header(VARY, "Origin")
            .body(crate::handler::full_body(Bytes::from(vec![b'a'; 640])))
            .expect("response should build");

        let response = maybe_encode_response(&Method::GET, &request_headers, response).await;
        let vary = response
            .headers()
            .get(VARY)
            .and_then(|value| value.to_str().ok())
            .expect("compressed response should keep vary");

        assert!(vary.contains("Origin"), "vary should retain existing dimensions: {vary}");
        assert!(
            vary.contains("Accept-Encoding"),
            "vary should advertise compression negotiation: {vary}"
        );
    }

    #[tokio::test]
    async fn maybe_encode_response_does_not_leave_stale_content_length_on_collect_failure() {
        let mut request_headers = HeaderMap::new();
        request_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));

        let response = Response::builder()
            .status(StatusCode::OK)
            .header(CONTENT_TYPE, "text/plain; charset=utf-8")
            .header(CONTENT_LENGTH, "512")
            .body(CollectErrorBody.map_err(|error| -> BoxError { Box::new(error) }).boxed_unsync())
            .expect("response should build");

        let response = maybe_encode_response(&Method::GET, &request_headers, response).await;
        let advertised_length = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok());

        match response.into_body().collect().await {
            Ok(collected) => {
                let body = collected.to_bytes();
                assert_eq!(
                    advertised_length.unwrap_or(body.len()),
                    body.len(),
                    "collect failure should not degrade into an empty body with stale content-length",
                );
            }
            Err(_) => {
                assert!(
                    advertised_length.is_none(),
                    "erroring fallback responses should not retain a stale content-length"
                );
            }
        }
    }
}
