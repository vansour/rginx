use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rcgen::{
    BasicConstraints, CertificateParams, CertificateRevocationList,
    CertificateRevocationListParams, DnType, IsCa, Issuer, KeyIdMethod, KeyPair, KeyUsagePurpose,
    RevocationReason, RevokedCertParams, SerialNumber, date_time_ymd,
};

use super::load_certificate_revocation_lists;

struct TestCertifiedKey {
    signing_key: KeyPair,
    params: CertificateParams,
}

impl TestCertifiedKey {
    fn issuer(&self) -> Issuer<'_, &KeyPair> {
        Issuer::from_params(&self.params, &self.signing_key)
    }
}

#[test]
fn load_certificate_revocation_lists_accepts_der_without_trailing_data() {
    let temp_dir = temp_dir("rginx-crl-der-valid");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let crl_path = temp_dir.join("client-auth.crl");

    let issuer = generate_ca_cert("rginx-crl-test-ca");
    let crl = generate_crl(&issuer, 42);
    fs::write(&crl_path, crl.der().as_ref()).expect("DER CRL should be written");

    let crls =
        load_certificate_revocation_lists(&crl_path).expect("DER CRL should load successfully");
    assert_eq!(crls.len(), 1);
    assert_eq!(crls[0].as_ref(), crl.der().as_ref());

    let _ = fs::remove_dir_all(temp_dir);
}

#[test]
fn load_certificate_revocation_lists_rejects_der_with_trailing_data() {
    let temp_dir = temp_dir("rginx-crl-der-trailing-data");
    fs::create_dir_all(&temp_dir).expect("temp dir should be created");
    let crl_path = temp_dir.join("client-auth.crl");

    let issuer = generate_ca_cert("rginx-crl-test-ca");
    let crl = generate_crl(&issuer, 42);
    let mut der = crl.der().as_ref().to_vec();
    der.extend_from_slice(b"trailing-bytes");
    fs::write(&crl_path, der).expect("DER CRL with trailing bytes should be written");

    let error = load_certificate_revocation_lists(&crl_path)
        .expect_err("DER CRL with trailing data should be rejected");
    assert!(error.to_string().contains("contains trailing data after the DER CRL payload"));

    let _ = fs::remove_dir_all(temp_dir);
}

fn generate_ca_cert(common_name: &str) -> TestCertifiedKey {
    let mut params =
        CertificateParams::new(vec![common_name.to_string()]).expect("CA params should build");
    params.distinguished_name.push(DnType::CommonName, common_name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    let signing_key = KeyPair::generate().expect("CA keypair should generate");
    let _cert = params.self_signed(&signing_key).expect("CA certificate should self-sign");
    TestCertifiedKey { signing_key, params }
}

fn generate_crl(issuer: &TestCertifiedKey, revoked_serial: u64) -> CertificateRevocationList {
    CertificateRevocationListParams {
        this_update: date_time_ymd(2024, 1, 1),
        next_update: date_time_ymd(2027, 1, 1),
        crl_number: SerialNumber::from(1),
        issuing_distribution_point: None,
        revoked_certs: vec![RevokedCertParams {
            serial_number: SerialNumber::from(revoked_serial),
            revocation_time: date_time_ymd(2024, 1, 2),
            reason_code: Some(RevocationReason::KeyCompromise),
            invalidity_date: None,
        }],
        key_identifier_method: KeyIdMethod::Sha256,
    }
    .signed_by(&issuer.issuer())
    .expect("CRL should be signed")
}

fn temp_dir(prefix: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}-{unique}-{id}"))
}
