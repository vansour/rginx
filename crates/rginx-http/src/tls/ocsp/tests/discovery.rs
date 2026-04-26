use super::*;

#[test]
fn ocsp_responder_urls_for_certificate_extracts_aia_ocsp_uri() {
    let temp_dir = temp_dir("rginx-ocsp-aia-responder-url");
    std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

    let ca = generate_ca_cert("ocsp-test-ca");
    let leaf = generate_leaf_cert_with_ocsp_aia("localhost", &ca, "http://127.0.0.1:19090/ocsp");
    let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);

    let urls = ocsp_responder_urls_for_certificate(&cert_path)
        .expect("AIA OCSP responder discovery should succeed");
    assert_eq!(urls, vec!["http://127.0.0.1:19090/ocsp".to_string()]);

    let _ = std::fs::remove_dir_all(temp_dir);
}
proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn ocsp_responder_discovery_handles_arbitrary_certificate_bytes_without_panicking(
        bytes in prop::collection::vec(any::<u8>(), 0..1024)
    ) {
        let temp_dir = temp_dir("rginx-ocsp-aia-proptest");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");
        let cert_path = temp_dir.join("arbitrary.crt");
        std::fs::write(&cert_path, &bytes).expect("arbitrary certificate bytes should be written");

        let result = ocsp_responder_urls_for_certificate(&cert_path);

        if let Ok(urls) = result {
            prop_assert!(urls.iter().all(|url| !url.is_empty()));
        }

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn validate_ocsp_response_rejects_corrupted_top_level_der_tags(
        tag in any::<u8>().prop_filter("DER sequence tag must change", |tag| *tag != 0x30)
    ) {
        let temp_dir = temp_dir("rginx-ocsp-corrupt-tag");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let leaf = generate_leaf_cert("localhost", &ca);
        let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
        let mut response = build_ocsp_response_for_certificate(&cert_path, &ca);
        response[0] = tag;

        let error = validate_ocsp_response_for_certificate(&cert_path, &response)
            .expect_err("corrupted top-level DER tag should be rejected");
        prop_assert!(error.to_string().contains("failed to parse OCSP response"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }
}
