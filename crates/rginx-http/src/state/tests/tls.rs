use super::*;

#[test]
fn inspect_certificate_reports_fingerprint_and_incomplete_chain_diagnostics() {
    let temp_dir = std::env::temp_dir().join("rginx-cert-inspect-test");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let cert_path = temp_dir.join("leaf.crt");

    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.distinguished_name.push(DnType::CommonName, "Test Root CA");
    let ca_key = KeyPair::generate().expect("CA key should generate");
    let _ca_cert = ca_params.self_signed(&ca_key).expect("CA should self-sign");
    let ca_issuer = Issuer::from_params(&ca_params, &ca_key);

    let mut leaf_params =
        CertificateParams::new(vec!["leaf.example.com".to_string()]).expect("leaf params");
    leaf_params.distinguished_name.push(DnType::CommonName, "leaf.example.com");
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf_key = KeyPair::generate().expect("leaf key should generate");
    let leaf_cert =
        leaf_params.signed_by(&leaf_key, &ca_issuer).expect("leaf should be signed by CA");

    fs::write(&cert_path, leaf_cert.pem()).expect("leaf cert should be written");

    let inspected = inspect_certificate(&cert_path).expect("certificate should be inspected");
    assert_eq!(inspected.subject.as_deref(), Some("CN=leaf.example.com"));
    assert_eq!(inspected.issuer.as_deref(), Some("CN=Test Root CA"));
    assert!(!inspected.san_dns_names.is_empty());
    assert!(inspected.fingerprint_sha256.as_ref().is_some_and(|value| value.len() == 64));
    assert_eq!(inspected.chain_length, 1);
    assert!(inspected.chain_diagnostics.iter().any(|diagnostic| {
        diagnostic.contains("chain_incomplete_single_non_self_signed_certificate")
    }));

    fs::remove_dir_all(temp_dir).expect("temp dir should be removed");
}

#[test]
fn inspect_certificate_reports_aki_ski_and_server_auth_eku_diagnostics() {
    let temp_dir = std::env::temp_dir().join("rginx-cert-inspect-extensions-test");
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let cert_path = temp_dir.join("leaf.crt");

    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.distinguished_name.push(DnType::CommonName, "Extension Root CA");
    let ca_key = KeyPair::generate().expect("CA key should generate");
    let _ca_cert = ca_params.self_signed(&ca_key).expect("CA should self-sign");
    let ca_issuer = Issuer::from_params(&ca_params, &ca_key);

    let mut leaf_params =
        CertificateParams::new(vec!["client-only.example.com".to_string()]).expect("leaf params");
    leaf_params.distinguished_name.push(DnType::CommonName, "client-only.example.com");
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
    let leaf_key = KeyPair::generate().expect("leaf key should generate");
    let leaf_cert =
        leaf_params.signed_by(&leaf_key, &ca_issuer).expect("leaf should be signed by CA");

    fs::write(&cert_path, leaf_cert.pem()).expect("leaf cert should be written");

    let inspected = inspect_certificate(&cert_path).expect("certificate should be inspected");
    assert!(inspected.extended_key_usage.iter().any(|usage| usage == "client_auth"));
    assert!(
        inspected
            .chain_diagnostics
            .iter()
            .any(|diagnostic| diagnostic == "leaf_missing_server_auth_eku")
    );

    fs::remove_dir_all(temp_dir).expect("temp dir should be removed");
}
