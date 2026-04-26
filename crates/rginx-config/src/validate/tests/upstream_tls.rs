use super::*;

#[test]
fn validate_rejects_partial_upstream_mtls_identity() {
    let mut config = base_config();
    config.upstreams[0].tls = Some(crate::model::UpstreamTlsConfig {
        verify: crate::model::UpstreamTlsModeConfig::NativeRoots,
        versions: None,
        verify_depth: None,
        crl_path: None,
        client_cert_path: Some("client.crt".to_string()),
        client_key_path: None,
    });

    let error = validate(&config).expect_err("partial upstream mTLS identity should fail");
    assert!(error.to_string().contains("requires both client_cert_path and client_key_path"));
}

#[test]
fn validate_rejects_zero_upstream_verify_depth() {
    let mut config = base_config();
    config.upstreams[0].peers[0].url = "https://example.com".to_string();
    config.upstreams[0].tls = Some(crate::model::UpstreamTlsConfig {
        verify: crate::model::UpstreamTlsModeConfig::NativeRoots,
        versions: None,
        verify_depth: Some(0),
        crl_path: None,
        client_cert_path: None,
        client_key_path: None,
    });

    let error = validate(&config).expect_err("zero upstream verify_depth should fail");
    assert!(error.to_string().contains("verify_depth must be greater than 0"));
}

#[test]
fn validate_rejects_upstream_crl_when_verification_is_disabled() {
    let mut config = base_config();
    config.upstreams[0].peers[0].url = "https://example.com".to_string();
    config.upstreams[0].tls = Some(crate::model::UpstreamTlsConfig {
        verify: crate::model::UpstreamTlsModeConfig::Insecure,
        versions: None,
        verify_depth: None,
        crl_path: Some("revocations.pem".to_string()),
        client_cert_path: None,
        client_key_path: None,
    });

    let error = validate(&config).expect_err("upstream CRL should require verification");
    assert!(
        error.to_string().contains("verify_depth and crl_path require certificate verification")
    );
}

#[test]
fn validate_accepts_upstream_verify_depth_and_crl_with_custom_ca() {
    let mut config = base_config();
    config.upstreams[0].peers[0].url = "https://example.com".to_string();
    config.upstreams[0].tls = Some(crate::model::UpstreamTlsConfig {
        verify: crate::model::UpstreamTlsModeConfig::CustomCa {
            ca_cert_path: "upstream-ca.pem".to_string(),
        },
        versions: None,
        verify_depth: Some(2),
        crl_path: Some("upstream.crl.pem".to_string()),
        client_cert_path: None,
        client_key_path: None,
    });

    validate(&config).expect("upstream verify_depth and CRL should validate");
}
