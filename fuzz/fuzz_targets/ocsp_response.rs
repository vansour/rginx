#![no_main]

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair};

static CERT_CHAIN_PATH: OnceLock<PathBuf> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let _ = rginx_http::validate_ocsp_response_for_certificate(ocsp_cert_chain_path(), data);
});

fn ocsp_cert_chain_path() -> &'static Path {
    CERT_CHAIN_PATH
        .get_or_init(|| {
            let root =
                std::env::temp_dir().join(format!("rginx-fuzz-ocsp-root-{}", std::process::id()));
            std::fs::create_dir_all(&root).expect("ocsp fuzz temp dir should be created");

            let ca = generate_ca_cert("rginx-fuzz-ocsp-ca");
            let leaf = generate_leaf_cert("localhost", &ca);
            let cert_path = root.join("server.crt");
            std::fs::write(&cert_path, format!("{}{}", leaf.cert.pem(), ca.cert.pem()))
                .expect("ocsp fuzz cert chain should be written");

            cert_path
        })
        .as_path()
}

struct TestCertifiedKey {
    cert: rcgen::Certificate,
    signing_key: KeyPair,
    params: CertificateParams,
}

impl TestCertifiedKey {
    fn issuer(&self) -> Issuer<'_, &KeyPair> {
        Issuer::from_params(&self.params, &self.signing_key)
    }
}

fn generate_ca_cert(common_name: &str) -> TestCertifiedKey {
    let mut params =
        CertificateParams::new(vec![common_name.to_string()]).expect("CA params should build");
    params.distinguished_name.push(DnType::CommonName, common_name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let signing_key = KeyPair::generate().expect("CA keypair should generate");
    let cert = params.self_signed(&signing_key).expect("CA certificate should self-sign");
    TestCertifiedKey { cert, signing_key, params }
}

fn generate_leaf_cert(common_name: &str, issuer: &TestCertifiedKey) -> TestCertifiedKey {
    let mut params =
        CertificateParams::new(vec![common_name.to_string()]).expect("leaf params should build");
    params.distinguished_name.push(DnType::CommonName, common_name);
    let signing_key = KeyPair::generate().expect("leaf keypair should generate");
    let cert = params
        .signed_by(&signing_key, &issuer.issuer())
        .expect("leaf certificate should be signed");
    TestCertifiedKey { cert, signing_key, params }
}
