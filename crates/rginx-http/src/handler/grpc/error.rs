use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use http::header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderMap, HeaderValue};
use http::{Response, StatusCode};

use crate::handler::{HttpResponse, full_body};

use super::metadata::grpc_protocol;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GrpcStatusCode {
    Cancelled,
    InvalidArgument,
    DeadlineExceeded,
    PermissionDenied,
    ResourceExhausted,
    Unimplemented,
    Unavailable,
}

impl GrpcStatusCode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Cancelled => "1",
            Self::InvalidArgument => "3",
            Self::DeadlineExceeded => "4",
            Self::PermissionDenied => "7",
            Self::ResourceExhausted => "8",
            Self::Unimplemented => "12",
            Self::Unavailable => "14",
        }
    }
}

#[derive(Debug, Clone)]
enum GrpcResponseFormat {
    Grpc { content_type: HeaderValue },
    GrpcWeb { content_type: HeaderValue, is_text: bool },
}

pub(crate) fn grpc_error_response(
    request_headers: &HeaderMap,
    grpc_status: GrpcStatusCode,
    message: &str,
) -> Option<HttpResponse> {
    let format = grpc_response_format(request_headers)?;
    Some(build_grpc_error_response(format, grpc_status, message))
}

fn grpc_response_format(headers: &HeaderMap) -> Option<GrpcResponseFormat> {
    let content_type = headers.get(CONTENT_TYPE)?.clone();
    match grpc_protocol(headers)? {
        "grpc" => Some(GrpcResponseFormat::Grpc { content_type }),
        "grpc-web" => Some(GrpcResponseFormat::GrpcWeb { content_type, is_text: false }),
        "grpc-web-text" => Some(GrpcResponseFormat::GrpcWeb { content_type, is_text: true }),
        _ => None,
    }
}

fn build_grpc_error_response(
    format: GrpcResponseFormat,
    grpc_status: GrpcStatusCode,
    message: &str,
) -> HttpResponse {
    let message = sanitize_grpc_message(message);
    let grpc_status_value = HeaderValue::from_static(grpc_status.as_str());
    let grpc_message_value = (!message.is_empty())
        .then(|| HeaderValue::from_str(&message).expect("sanitized gRPC message should be valid"));

    match format {
        GrpcResponseFormat::Grpc { content_type } => {
            let mut builder = Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, content_type)
                .header(CONTENT_LENGTH, "0")
                .header("grpc-status", grpc_status_value);
            if let Some(message) = grpc_message_value {
                builder = builder.header("grpc-message", message);
            }
            builder
                .body(full_body(Bytes::new()))
                .expect("gRPC error response builder should not fail")
        }
        GrpcResponseFormat::GrpcWeb { content_type, is_text } => {
            let mut trailers = HeaderMap::new();
            trailers.insert("grpc-status", grpc_status_value.clone());
            if let Some(message) = grpc_message_value.clone() {
                trailers.insert("grpc-message", message);
            }

            let body = encode_grpc_web_error_body(&trailers, is_text);
            let mut builder = Response::builder()
                .status(StatusCode::OK)
                .header(CONTENT_TYPE, content_type)
                .header(CONTENT_LENGTH, body.len().to_string())
                .header("grpc-status", grpc_status_value);
            if let Some(message) = grpc_message_value {
                builder = builder.header("grpc-message", message);
            }
            builder.body(full_body(body)).expect("grpc-web error response builder should not fail")
        }
    }
}

fn sanitize_grpc_message(message: &str) -> String {
    message
        .trim()
        .chars()
        .map(|ch| if ch.is_ascii_control() { ' ' } else { ch })
        .collect::<String>()
}

fn encode_grpc_web_error_body(trailers: &HeaderMap, is_text: bool) -> Bytes {
    let mut trailer_block = Vec::new();
    for (name, value) in trailers {
        trailer_block.extend_from_slice(name.as_str().as_bytes());
        trailer_block.extend_from_slice(b": ");
        trailer_block.extend_from_slice(value.as_bytes());
        trailer_block.extend_from_slice(b"\r\n");
    }

    let mut encoded = Vec::with_capacity(5 + trailer_block.len());
    encoded.push(0x80);
    encoded.extend_from_slice(&(trailer_block.len() as u32).to_be_bytes());
    encoded.extend_from_slice(&trailer_block);

    if is_text { Bytes::from(STANDARD.encode(&encoded)) } else { Bytes::from(encoded) }
}
