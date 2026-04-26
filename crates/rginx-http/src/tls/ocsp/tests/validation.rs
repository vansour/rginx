use super::*;

#[test]
fn validate_ocsp_response_matches_current_certificate() {
    let temp_dir = temp_dir("rginx-ocsp-validate");
    std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    let ca = generate_ca_cert("ocsp-test-ca");
    let leaf = generate_leaf_cert("localhost", &ca);
    let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
    let response = build_ocsp_response_for_certificate(&cert_path, &ca);

    validate_ocsp_response_for_certificate(&cert_path, &response)
        .expect("OCSP response should match the current certificate");

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn validate_ocsp_response_rejects_expired_response() {
    let temp_dir = temp_dir("rginx-ocsp-expired");
    std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    let ca = generate_ca_cert("ocsp-test-ca");
    let leaf = generate_leaf_cert("localhost", &ca);
    let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
    let response = build_ocsp_response_for_certificate_with_offsets(
        &cert_path,
        &ca,
        TimeOffset::Before(Duration::from_secs(2 * 24 * 60 * 60)),
        TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
    );

    let error = validate_ocsp_response_for_certificate(&cert_path, &response)
        .expect_err("expired OCSP response should be rejected");
    assert!(error.to_string().contains("is expired"));

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn load_certified_key_bundle_ignores_stale_ocsp_cache() {
    let temp_dir = temp_dir("rginx-ocsp-stale-cache");
    std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    let ca = generate_ca_cert("ocsp-test-ca");
    let current_leaf = generate_leaf_cert("localhost", &ca);
    let stale_leaf = generate_leaf_cert("localhost", &ca);
    let cert_path = write_cert_chain(&temp_dir, "current", &current_leaf, &ca);
    let key_path = write_private_key(&temp_dir, "current", &current_leaf);
    let stale_cert_path = write_cert_chain(&temp_dir, "stale", &stale_leaf, &ca);
    let ocsp_path = temp_dir.join("server.ocsp");
    std::fs::write(&ocsp_path, build_ocsp_response_for_certificate(&stale_cert_path, &ca))
        .expect("stale OCSP response should be written");

    let bundle = ServerCertificateBundle {
        cert_path,
        key_path,
        ocsp_staple_path: Some(ocsp_path),
        ocsp: rginx_core::OcspConfig::default(),
    };
    let certified_key = load_certified_key_bundle(&bundle)
        .expect("certificate bundle should still load without reusing stale OCSP data");
    assert!(certified_key.ocsp.is_none(), "stale OCSP response should not be stapled");

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn validate_ocsp_response_rejects_future_produced_at() {
    let temp_dir = temp_dir("rginx-ocsp-produced-at-future");
    std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    let ca = generate_ca_cert("ocsp-test-ca");
    let leaf = generate_leaf_cert("localhost", &ca);
    let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
    let response = build_ocsp_response_for_certificate_with_signer(
        &cert_path,
        OcspResponseOptions::new(OcspResponseSigner::Issuer(&ca))
            .this_update_offset(TimeOffset::Before(Duration::from_secs(60)))
            .produced_at_offset(TimeOffset::After(Duration::from_secs(60))),
    );

    let error = validate_ocsp_response_for_certificate(&cert_path, &response)
        .expect_err("future producedAt should be rejected");
    assert!(error.to_string().contains("producedAt is in the future"));

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn validate_ocsp_response_rejects_unknown_certificate_status() {
    let temp_dir = temp_dir("rginx-ocsp-unknown-status");
    std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    let ca = generate_ca_cert("ocsp-test-ca");
    let leaf = generate_leaf_cert("localhost", &ca);
    let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
    let response = build_ocsp_response_for_certificate_with_signer(
        &cert_path,
        OcspResponseOptions::new(OcspResponseSigner::Issuer(&ca))
            .cert_status(RasnCertStatus::Unknown(())),
    );

    let error = validate_ocsp_response_for_certificate(&cert_path, &response)
        .expect_err("unknown OCSP cert status should be rejected");
    assert!(error.to_string().contains("unknown certificate status"));

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn validate_ocsp_response_rejects_invalid_signature() {
    let temp_dir = temp_dir("rginx-ocsp-invalid-signature");
    std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    let ca = generate_ca_cert("ocsp-test-ca");
    let leaf = generate_leaf_cert("localhost", &ca);
    let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
    let response = build_ocsp_response_for_certificate_with_signer(
        &cert_path,
        OcspResponseOptions::new(OcspResponseSigner::Issuer(&ca)).tamper_signature(true),
    );

    let error = validate_ocsp_response_for_certificate(&cert_path, &response)
        .expect_err("invalid OCSP signature should be rejected");
    assert!(error.to_string().contains("invalid responder signature"));

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn validate_ocsp_response_accepts_authorized_delegated_signer() {
    let temp_dir = temp_dir("rginx-ocsp-delegated-signer");
    std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    let ca = generate_ca_cert("ocsp-test-ca");
    let leaf = generate_leaf_cert("localhost", &ca);
    let responder = generate_ocsp_responder_cert("ocsp-responder", &ca, true, true);
    let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
    let response = build_ocsp_response_for_certificate_with_signer(
        &cert_path,
        OcspResponseOptions::new(OcspResponseSigner::Delegated(&responder)),
    );

    validate_ocsp_response_for_certificate(&cert_path, &response)
        .expect("authorized delegated responder should be accepted");

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn validate_ocsp_response_rejects_delegated_signer_without_ocsp_eku() {
    let temp_dir = temp_dir("rginx-ocsp-delegated-no-eku");
    std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    let ca = generate_ca_cert("ocsp-test-ca");
    let leaf = generate_leaf_cert("localhost", &ca);
    let responder = generate_ocsp_responder_cert("ocsp-responder", &ca, false, true);
    let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
    let response = build_ocsp_response_for_certificate_with_signer(
        &cert_path,
        OcspResponseOptions::new(OcspResponseSigner::Delegated(&responder)),
    );

    let error = validate_ocsp_response_for_certificate(&cert_path, &response)
        .expect_err("delegated responder without EKU should be rejected");
    assert!(error.to_string().contains("OCSP signing extended key usage"));

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[test]
fn validate_ocsp_response_rejects_multiple_matching_single_responses() {
    let temp_dir = temp_dir("rginx-ocsp-duplicate-matches");
    std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    let ca = generate_ca_cert("ocsp-test-ca");
    let leaf = generate_leaf_cert("localhost", &ca);
    let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
    let response = build_ocsp_response_for_certificate_with_signer(
        &cert_path,
        OcspResponseOptions::new(OcspResponseSigner::Issuer(&ca)).duplicate_matching_response(true),
    );

    let error = validate_ocsp_response_for_certificate(&cert_path, &response)
        .expect_err("multiple matching SingleResponses should be rejected");
    assert!(error.to_string().contains("multiple matching SingleResponses"));

    let _ = std::fs::remove_dir_all(temp_dir);
}
