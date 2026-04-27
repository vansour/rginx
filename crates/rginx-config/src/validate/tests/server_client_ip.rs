use super::*;

#[test]
fn validate_rejects_invalid_client_ip_header() {
    let mut config = base_config();
    config.server.client_ip_header = Some("bad header".to_string());

    let error = validate(&config).expect_err("invalid client_ip_header should be rejected");
    assert!(error.to_string().contains("server client_ip_header `bad header` is invalid"));
}
