use super::*;

#[test]
fn validate_rejects_zero_server_max_connections() {
    let mut config = base_config();
    config.server.max_connections = Some(0);

    let error = validate(&config).expect_err("zero max connections should be rejected");
    assert!(error.to_string().contains("server max_connections must be greater than 0"));
}

#[test]
fn validate_rejects_zero_server_header_read_timeout() {
    let mut config = base_config();
    config.server.header_read_timeout_secs = Some(0);

    let error = validate(&config).expect_err("zero header timeout should be rejected");
    assert!(error.to_string().contains("server header_read_timeout_secs must be greater than 0"));
}

#[test]
fn validate_rejects_zero_server_request_body_read_timeout() {
    let mut config = base_config();
    config.server.request_body_read_timeout_secs = Some(0);

    let error = validate(&config).expect_err("zero request body read timeout should be rejected");
    assert!(
        error.to_string().contains("server request_body_read_timeout_secs must be greater than 0")
    );
}

#[test]
fn validate_rejects_zero_server_response_write_timeout() {
    let mut config = base_config();
    config.server.response_write_timeout_secs = Some(0);

    let error = validate(&config).expect_err("zero response write timeout should be rejected");
    assert!(
        error.to_string().contains("server response_write_timeout_secs must be greater than 0")
    );
}

#[test]
fn validate_rejects_zero_server_max_headers() {
    let mut config = base_config();
    config.server.max_headers = Some(0);

    let error = validate(&config).expect_err("zero max headers should be rejected");
    assert!(error.to_string().contains("server max_headers must be greater than 0"));
}

#[test]
fn validate_rejects_zero_server_max_request_body_bytes() {
    let mut config = base_config();
    config.server.max_request_body_bytes = Some(0);

    let error = validate(&config).expect_err("zero max request body should be rejected");
    assert!(error.to_string().contains("server max_request_body_bytes must be greater than 0"));
}
