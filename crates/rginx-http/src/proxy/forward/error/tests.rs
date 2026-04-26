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
