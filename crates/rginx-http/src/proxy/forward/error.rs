use std::error::Error as StdError;
use std::future::Future;

use super::super::request_body::request_body_limit_error;
use super::*;

pub(super) fn invalid_downstream_request_body_error(error: &(dyn StdError + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(candidate) = current {
        if let Some(io_error) = candidate.downcast_ref::<std::io::Error>()
            && matches!(
                io_error.kind(),
                std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput
            )
        {
            return true;
        }

        let message = candidate.to_string();
        if message.contains("grpc-web")
            && (message.contains("invalid") || message.contains("incomplete"))
        {
            return true;
        }

        current = candidate.source();
    }

    false
}

pub(super) fn downstream_request_body_limit(error: &(dyn StdError + 'static)) -> Option<usize> {
    let mut current = Some(error);
    while let Some(candidate) = current {
        if let Some(limit) = request_body_limit_error(candidate) {
            return Some(limit);
        }

        if let Some(limit) = parse_request_body_limit_message(&candidate.to_string()) {
            return Some(limit);
        }

        current = candidate.source();
    }

    None
}

fn parse_request_body_limit_message(message: &str) -> Option<usize> {
    let prefix = "request body exceeded configured limit of ";
    let suffix = " bytes";
    let start = message.find(prefix)? + prefix.len();
    let rest = &message[start..];
    let end = rest.find(suffix)?;
    let value = &rest[..end];
    value.parse().ok()
}

pub(crate) async fn wait_for_upstream_stage<T>(
    request_timeout: Duration,
    upstream_name: &str,
    stage: &str,
    future: impl Future<Output = T>,
) -> Result<T, Error> {
    tokio::time::timeout(request_timeout, future).await.map_err(|_| {
        Error::Server(format!(
            "upstream `{upstream_name}` {stage} timed out after {} ms",
            request_timeout.as_millis()
        ))
    })
}

pub(super) fn gateway_timeout(request_headers: &HeaderMap, message: String) -> HttpResponse {
    grpc_error_response(request_headers, GrpcStatusCode::DeadlineExceeded, &message).unwrap_or_else(
        || {
            crate::handler::text_response(
                StatusCode::GATEWAY_TIMEOUT,
                "text/plain; charset=utf-8",
                message,
            )
        },
    )
}

pub(super) fn grpc_timeout_message(upstream_name: &str, timeout: Duration) -> String {
    format!("upstream `{upstream_name}` timed out after {} ms", timeout.as_millis())
}

pub(super) fn bad_gateway(request_headers: &HeaderMap, message: String) -> HttpResponse {
    grpc_error_response(request_headers, GrpcStatusCode::Unavailable, &message).unwrap_or_else(
        || {
            crate::handler::text_response(
                StatusCode::BAD_GATEWAY,
                "text/plain; charset=utf-8",
                message,
            )
        },
    )
}

pub(super) fn payload_too_large(request_headers: &HeaderMap, message: String) -> HttpResponse {
    grpc_error_response(request_headers, GrpcStatusCode::ResourceExhausted, &message)
        .unwrap_or_else(|| {
            crate::handler::text_response(
                StatusCode::PAYLOAD_TOO_LARGE,
                "text/plain; charset=utf-8",
                message,
            )
        })
}

pub(super) fn bad_request(request_headers: &HeaderMap, message: String) -> HttpResponse {
    grpc_error_response(request_headers, GrpcStatusCode::InvalidArgument, &message).unwrap_or_else(
        || {
            crate::handler::text_response(
                StatusCode::BAD_REQUEST,
                "text/plain; charset=utf-8",
                message,
            )
        },
    )
}

pub(super) fn unsupported_media_type(request_headers: &HeaderMap, message: String) -> HttpResponse {
    grpc_error_response(request_headers, GrpcStatusCode::InvalidArgument, &message).unwrap_or_else(
        || {
            crate::handler::text_response(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "text/plain; charset=utf-8",
                message,
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn detects_invalid_io_kinds_in_error_chain() {
        let error = io::Error::new(io::ErrorKind::InvalidData, "bad request body");
        assert!(invalid_downstream_request_body_error(&error));
    }

    #[test]
    fn detects_stringified_incomplete_grpc_web_errors() {
        let error = Error::Server(
            "upstream request failed: client error (SendRequest): error from user's Body stream: incomplete grpc-web-text base64 body"
                .to_string(),
        );
        assert!(invalid_downstream_request_body_error(&error));
    }

    #[test]
    fn ignores_unrelated_server_errors() {
        let error = Error::Server("upstream `backend` is unavailable".to_string());
        assert!(!invalid_downstream_request_body_error(&error));
    }

    #[test]
    fn extracts_request_body_limit_from_stringified_server_errors() {
        let error = Error::Server(
            "upstream request failed: client error (SendRequest): error from user's Body stream: request body exceeded configured limit of 8 bytes"
                .to_string(),
        );
        assert_eq!(downstream_request_body_limit(&error), Some(8));
    }
}
