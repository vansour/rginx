use super::*;

#[test]
fn build_ocsp_request_includes_nonce_when_enabled() {
    let temp_dir = temp_dir("rginx-ocsp-request-nonce");
    std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    let ca = generate_ca_cert("ocsp-test-ca");
    let leaf = generate_leaf_cert("localhost", &ca);
    let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
    let (request, nonce) =
        build_ocsp_request_for_certificate_with_options(&cert_path, OcspNonceMode::Required)
            .expect("OCSP request should build with nonce");
    let request: RasnOcspRequest = rasn::der::decode(&request).expect("OCSP request should decode");

    let request_nonce =
        extract_ocsp_nonce(&cert_path, request.tbs_request.request_extensions.as_ref())
            .expect("request nonce should parse")
            .expect("request nonce should exist");
    assert_eq!(request_nonce, nonce.expect("nonce should be generated"));

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn validate_ocsp_response_rejects_missing_required_nonce() {
    let temp_dir = temp_dir("rginx-ocsp-missing-required-nonce");
    std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    let ca = generate_ca_cert("ocsp-test-ca");
    let leaf = generate_leaf_cert("localhost", &ca);
    let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
    let response = build_ocsp_response_for_certificate_with_signer(
        &cert_path,
        TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
        Some(TimeOffset::After(Duration::from_secs(24 * 60 * 60))),
        TimeOffset::Before(Duration::from_secs(60)),
        RasnCertStatus::Good,
        OcspResponseSigner::Issuer(&ca),
        None,
        false,
        false,
    );

    let error = validate_ocsp_response_for_certificate_with_options(
        &cert_path,
        &response,
        Some(b"expected-nonce"),
        OcspNonceMode::Required,
        OcspResponderPolicy::IssuerOrDelegated,
    )
    .expect_err("missing required nonce should be rejected");
    assert!(error.to_string().contains("did not echo the required nonce"));

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn validate_ocsp_response_rejects_mismatched_required_nonce() {
    let temp_dir = temp_dir("rginx-ocsp-mismatched-required-nonce");
    std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    let ca = generate_ca_cert("ocsp-test-ca");
    let leaf = generate_leaf_cert("localhost", &ca);
    let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
    let response = build_ocsp_response_for_certificate_with_signer(
        &cert_path,
        TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
        Some(TimeOffset::After(Duration::from_secs(24 * 60 * 60))),
        TimeOffset::Before(Duration::from_secs(60)),
        RasnCertStatus::Good,
        OcspResponseSigner::Issuer(&ca),
        Some(b"response-nonce"),
        false,
        false,
    );

    let error = validate_ocsp_response_for_certificate_with_options(
        &cert_path,
        &response,
        Some(b"expected-nonce"),
        OcspNonceMode::Required,
        OcspResponderPolicy::IssuerOrDelegated,
    )
    .expect_err("mismatched nonce should be rejected");
    assert!(error.to_string().contains("mismatched nonce"));

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn validate_ocsp_response_accepts_missing_preferred_nonce() {
    let temp_dir = temp_dir("rginx-ocsp-preferred-nonce-missing");
    std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    let ca = generate_ca_cert("ocsp-test-ca");
    let leaf = generate_leaf_cert("localhost", &ca);
    let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
    let response = build_ocsp_response_for_certificate_with_signer(
        &cert_path,
        TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
        Some(TimeOffset::After(Duration::from_secs(24 * 60 * 60))),
        TimeOffset::Before(Duration::from_secs(60)),
        RasnCertStatus::Good,
        OcspResponseSigner::Issuer(&ca),
        None,
        false,
        false,
    );

    validate_ocsp_response_for_certificate_with_options(
        &cert_path,
        &response,
        Some(b"expected-nonce"),
        OcspNonceMode::Preferred,
        OcspResponderPolicy::IssuerOrDelegated,
    )
    .expect("preferred nonce should allow missing echoed nonce");

    let _ = std::fs::remove_dir_all(temp_dir);
}
