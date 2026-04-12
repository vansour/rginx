use std::path::Path;
use std::sync::Arc;

use rginx_core::{Error, Result, ServerCertificateBundle, ServerTls, VirtualHostTls};
use rustls::RootCertStore;
use rustls::pki_types::{
    CertificateDer, CertificateRevocationListDer, PrivateKeyDer,
    pem::{Error as PemError, PemObject},
};

use crate::pki::validate_der_certificate_revocation_list;

pub(crate) fn load_certified_keys(tls: &ServerTls) -> Result<Vec<Arc<rustls::sign::CertifiedKey>>> {
    load_certified_keys_from_material(
        &tls.cert_path,
        &tls.key_path,
        &tls.additional_certificates,
        tls.ocsp_staple_path.as_ref(),
        &tls.ocsp,
    )
}

pub(crate) fn load_vhost_certified_keys(
    tls: &VirtualHostTls,
) -> Result<Vec<Arc<rustls::sign::CertifiedKey>>> {
    load_certified_keys_from_material(
        &tls.cert_path,
        &tls.key_path,
        &tls.additional_certificates,
        tls.ocsp_staple_path.as_ref(),
        &tls.ocsp,
    )
}

fn load_certified_keys_from_material(
    cert_path: &Path,
    key_path: &Path,
    additional_certificates: &[ServerCertificateBundle],
    ocsp_staple_path: Option<&std::path::PathBuf>,
    ocsp: &rginx_core::OcspConfig,
) -> Result<Vec<Arc<rustls::sign::CertifiedKey>>> {
    let mut bundles = Vec::with_capacity(1 + additional_certificates.len());
    bundles.push(ServerCertificateBundle {
        cert_path: cert_path.to_path_buf(),
        key_path: key_path.to_path_buf(),
        ocsp_staple_path: ocsp_staple_path.cloned(),
        ocsp: ocsp.clone(),
    });
    bundles.extend(additional_certificates.iter().cloned());

    bundles.into_iter().map(|bundle| load_certified_key_bundle(&bundle)).collect()
}

pub(crate) fn load_certified_key_bundle(
    bundle: &ServerCertificateBundle,
) -> Result<Arc<rustls::sign::CertifiedKey>> {
    let certs = load_certificate_chain_from_path(&bundle.cert_path)?;
    let key = load_private_key_from_path(&bundle.key_path)?;

    let mut certified_key = rustls::sign::CertifiedKey::new(
        certs,
        rustls::crypto::aws_lc_rs::sign::any_supported_type(&key).map_err(|_| {
            Error::Server(format!(
                "server TLS private key file `{}` uses unsupported algorithm",
                bundle.key_path.display()
            ))
        })?,
    );

    if let Some(ocsp_staple_path) = &bundle.ocsp_staple_path {
        let ocsp = std::fs::read(ocsp_staple_path)?;
        if !ocsp.is_empty() {
            match super::ocsp::validate_ocsp_response_for_certificate_with_options(
                &bundle.cert_path,
                &ocsp,
                None,
                rginx_core::OcspNonceMode::Disabled,
                bundle.ocsp.responder_policy,
            ) {
                Ok(()) => {
                    certified_key.ocsp = Some(ocsp);
                }
                Err(error) => {
                    tracing::warn!(
                        cert_path = %bundle.cert_path.display(),
                        staple_path = %ocsp_staple_path.display(),
                        %error,
                        "ignoring invalid OCSP staple cache file"
                    );
                }
            }
        }
    }

    certified_key.keys_match().map_err(|error| {
        Error::Server(format!(
            "server TLS certificate `{}` does not match private key `{}`: {error}",
            bundle.cert_path.display(),
            bundle.key_path.display()
        ))
    })?;

    Ok(Arc::new(certified_key))
}

pub(crate) fn load_certificate_chain_from_path(
    path: &Path,
) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let certs = CertificateDer::pem_file_iter(path)
        .map_err(|error| map_pem_error(path, "certificates", error))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| map_pem_error(path, "certificates", error))?;

    if certs.is_empty() {
        return Err(Error::Server(format!(
            "server TLS certificate file `{}` did not contain any PEM certificates",
            path.display()
        )));
    }

    Ok(certs)
}

pub(crate) fn load_ca_cert_store(path: &Path) -> Result<RootCertStore> {
    let certs = CertificateDer::pem_file_iter(path)
        .map_err(|error| map_pem_error(path, "CA certificates", error))?
        .collect::<std::result::Result<Vec<CertificateDer<'static>>, _>>()
        .map_err(|error| map_pem_error(path, "CA certificates", error))?;

    let mut roots = RootCertStore::empty();
    if certs.is_empty() {
        let der = std::fs::read(path)?;
        roots.add(CertificateDer::from(der)).map_err(|error| {
            Error::Server(format!("failed to add DER CA certificate `{}`: {error}", path.display()))
        })?;
        return Ok(roots);
    }

    let (added, _ignored) = roots.add_parsable_certificates(certs);
    if added == 0 || roots.is_empty() {
        return Err(Error::Server(format!(
            "no valid CA certificates were loaded from `{}`",
            path.display()
        )));
    }

    Ok(roots)
}

pub(crate) fn load_certificate_revocation_lists(
    path: &Path,
) -> Result<Vec<CertificateRevocationListDer<'static>>> {
    let crls = CertificateRevocationListDer::pem_file_iter(path)
        .map_err(|error| map_pem_error(path, "certificate revocation lists", error))?
        .collect::<std::result::Result<Vec<CertificateRevocationListDer<'static>>, _>>()
        .map_err(|error| map_pem_error(path, "certificate revocation lists", error))?;

    if !crls.is_empty() {
        return Ok(crls);
    }

    let der = std::fs::read(path)?;
    validate_der_crl(path, &der)?;
    Ok(vec![CertificateRevocationListDer::from(der)])
}

fn validate_der_crl(path: &Path, der: &[u8]) -> Result<()> {
    validate_der_certificate_revocation_list(path, der)
}

pub(crate) fn load_private_key_from_path(
    path: &Path,
) -> Result<rustls::pki_types::PrivateKeyDer<'static>> {
    match PrivateKeyDer::from_pem_file(path) {
        Ok(key) => Ok(key),
        Err(PemError::NoItemsFound) => Err(Error::Server(format!(
            "server TLS private key file `{}` did not contain a supported PEM private key",
            path.display()
        ))),
        Err(error) => Err(map_pem_error(path, "private key", error)),
    }
}

fn map_pem_error(path: &Path, item: &str, error: PemError) -> Error {
    match error {
        PemError::Io(error) => Error::Io(error),
        other => {
            Error::Server(format!("failed to parse {item} from `{}`: {other}", path.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use rcgen::{
        BasicConstraints, CertificateParams, CertificateRevocationList,
        CertificateRevocationListParams, DnType, IsCa, Issuer, KeyIdMethod, KeyPair,
        KeyUsagePurpose, RevocationReason, RevokedCertParams, SerialNumber, date_time_ymd,
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
}
