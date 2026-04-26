use http::header::{CONTENT_ENCODING, CONTENT_RANGE, CONTENT_TYPE};
use http::{HeaderMap, StatusCode};

use super::ResponseCompressionOptions;

pub(super) fn response_is_eligible(
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
