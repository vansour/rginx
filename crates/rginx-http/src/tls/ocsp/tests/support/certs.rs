use super::*;

pub(crate) struct TestCertifiedKey {
    pub(crate) cert: rcgen::Certificate,
    pub(crate) signing_key: KeyPair,
    pub(crate) params: CertificateParams,
}

impl TestCertifiedKey {
    pub(crate) fn issuer(&self) -> Issuer<'_, &KeyPair> {
        Issuer::from_params(&self.params, &self.signing_key)
    }
}

pub(crate) fn generate_ca_cert(common_name: &str) -> TestCertifiedKey {
    let mut params =
        CertificateParams::new(vec![common_name.to_string()]).expect("CA params should build");
    params.distinguished_name.push(DnType::CommonName, common_name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let signing_key = KeyPair::generate().expect("CA keypair should generate");
    let cert = params.self_signed(&signing_key).expect("CA certificate should self-sign");
    TestCertifiedKey { cert, signing_key, params }
}

pub(crate) fn generate_leaf_cert(common_name: &str, issuer: &TestCertifiedKey) -> TestCertifiedKey {
    let mut params =
        CertificateParams::new(vec![common_name.to_string()]).expect("leaf params should build");
    params.distinguished_name.push(DnType::CommonName, common_name);
    let signing_key = KeyPair::generate().expect("leaf keypair should generate");
    let cert = params
        .signed_by(&signing_key, &issuer.issuer())
        .expect("leaf certificate should be signed");
    TestCertifiedKey { cert, signing_key, params }
}

pub(crate) fn generate_leaf_cert_with_ocsp_aia(
    common_name: &str,
    issuer: &TestCertifiedKey,
    responder_url: &str,
) -> TestCertifiedKey {
    let mut params =
        CertificateParams::new(vec![common_name.to_string()]).expect("leaf params should build");
    params.distinguished_name.push(DnType::CommonName, common_name);
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.custom_extensions.push(CustomExtension::from_oid_content(
        &[1, 3, 6, 1, 5, 5, 7, 1, 1],
        authority_info_access_extension_value(responder_url),
    ));
    let signing_key = KeyPair::generate().expect("leaf keypair should generate");
    let cert = params
        .signed_by(&signing_key, &issuer.issuer())
        .expect("leaf certificate should be signed");
    TestCertifiedKey { cert, signing_key, params }
}

pub(crate) fn generate_ocsp_responder_cert(
    common_name: &str,
    issuer: &TestCertifiedKey,
    ocsp_signing: bool,
    digital_signature: bool,
) -> TestCertifiedKey {
    let mut params = CertificateParams::new(vec![common_name.to_string()])
        .expect("OCSP responder params should build");
    params.distinguished_name.push(DnType::CommonName, common_name);
    if ocsp_signing {
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::OcspSigning];
    }
    if digital_signature {
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    }
    let signing_key = KeyPair::generate().expect("OCSP responder keypair should generate");
    let cert = params
        .signed_by(&signing_key, &issuer.issuer())
        .expect("OCSP responder certificate should be signed");
    TestCertifiedKey { cert, signing_key, params }
}

pub(crate) fn write_cert_chain(
    temp_dir: &Path,
    name: &str,
    leaf: &TestCertifiedKey,
    ca: &TestCertifiedKey,
) -> PathBuf {
    let path = temp_dir.join(format!("{name}.crt"));
    std::fs::write(&path, format!("{}{}", leaf.cert.pem(), ca.cert.pem()))
        .expect("certificate chain should be written");
    path
}

pub(crate) fn write_private_key(temp_dir: &Path, name: &str, leaf: &TestCertifiedKey) -> PathBuf {
    let path = temp_dir.join(format!("{name}.key"));
    std::fs::write(&path, leaf.signing_key.serialize_pem()).expect("private key should be written");
    path
}
