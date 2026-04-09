use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use der::{Decode, Encode};
use rginx_core::{Error, Result, ServerCertificateBundle, ServerTls, VirtualHostTls};
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, CertificateRevocationListDer};
use sha1::{Digest, Sha1};
use x509_ocsp::{BasicOcspResponse, OcspResponse, OcspResponseStatus};
use x509_parser::extensions::ParsedExtension;
use x509_parser::prelude::{FromDer, X509Certificate};

pub(crate) fn load_certified_keys(tls: &ServerTls) -> Result<Vec<Arc<rustls::sign::CertifiedKey>>> {
    load_certified_keys_from_material(
        &tls.cert_path,
        &tls.key_path,
        &tls.additional_certificates,
        tls.ocsp_staple_path.as_ref(),
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
    )
}

fn load_certified_keys_from_material(
    cert_path: &Path,
    key_path: &Path,
    additional_certificates: &[ServerCertificateBundle],
    ocsp_staple_path: Option<&std::path::PathBuf>,
) -> Result<Vec<Arc<rustls::sign::CertifiedKey>>> {
    let mut bundles = Vec::with_capacity(1 + additional_certificates.len());
    bundles.push(ServerCertificateBundle {
        cert_path: cert_path.to_path_buf(),
        key_path: key_path.to_path_buf(),
        ocsp_staple_path: ocsp_staple_path.cloned(),
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
        if !ocsp.is_empty()
            && validate_ocsp_response_for_certificate(&bundle.cert_path, &ocsp).is_ok()
        {
            certified_key.ocsp = Some(ocsp);
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

pub(crate) fn ocsp_responder_urls_for_certificate(path: &Path) -> Result<Vec<String>> {
    let certs = load_certificate_chain_from_path(path)?;
    let Some(leaf) = certs.first() else {
        return Ok(Vec::new());
    };

    let (_, cert) = X509Certificate::from_der(leaf.as_ref()).map_err(|error| {
        Error::Server(format!(
            "failed to parse X.509 certificate `{}` for OCSP responder discovery: {error}",
            path.display()
        ))
    })?;
    Ok(ocsp_responder_urls_from_cert(&cert))
}

pub(crate) fn build_ocsp_request_for_certificate(path: &Path) -> Result<Vec<u8>> {
    let certs = load_certificate_chain_from_path(path)?;
    build_ocsp_request_from_chain(&certs, path)
}

pub(crate) fn validate_ocsp_response_for_certificate(
    path: &Path,
    response_der: &[u8],
) -> Result<()> {
    let certs = load_certificate_chain_from_path(path)?;
    let expected_cert_id = build_ocsp_cert_id_from_chain(&certs, path)?;

    let response = OcspResponse::from_der(response_der).map_err(|error| {
        Error::Server(format!(
            "failed to parse OCSP response for certificate `{}`: {error}",
            path.display()
        ))
    })?;
    if response.response_status != OcspResponseStatus::Successful {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` is not successful: {:?}",
            path.display(),
            response.response_status
        )));
    }

    let response_bytes = response.response_bytes.ok_or_else(|| {
        Error::Server(format!(
            "OCSP response for certificate `{}` is missing response_bytes",
            path.display()
        ))
    })?;
    if response_bytes.response_type.to_string() != "1.3.6.1.5.5.7.48.1.1" {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` uses unsupported response type `{}`",
            path.display(),
            response_bytes.response_type
        )));
    }

    let basic_response =
        BasicOcspResponse::from_der(response_bytes.response.as_bytes()).map_err(|error| {
            Error::Server(format!(
                "failed to parse basic OCSP response for certificate `{}`: {error}",
                path.display()
            ))
        })?;
    let mut matched_response = false;
    let mut last_time_error = None;
    for response in &basic_response.tbs_response_data.responses {
        let matches_certificate =
            response.cert_id.to_der().map(|cert_id| cert_id == expected_cert_id).unwrap_or(false);
        if !matches_certificate {
            continue;
        }
        matched_response = true;
        match validate_ocsp_response_time(path, response, SystemTime::now()) {
            Ok(()) => return Ok(()),
            Err(error) => last_time_error = Some(error),
        }
    }
    if !matched_response {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` does not contain a matching SingleResponse",
            path.display()
        )));
    }

    Err(last_time_error.unwrap_or_else(|| {
        Error::Server(format!(
            "OCSP response for certificate `{}` did not contain a currently valid SingleResponse",
            path.display()
        ))
    }))
}

pub(crate) fn load_certificate_chain_from_path(
    path: &Path,
) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(Error::Io)?;

    if certs.is_empty() {
        return Err(Error::Server(format!(
            "server TLS certificate file `{}` did not contain any PEM certificates",
            path.display()
        )));
    }

    Ok(certs)
}

pub(crate) fn load_ca_cert_store(path: &Path) -> Result<RootCertStore> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<CertificateDer<'static>>, _>>()
        .map_err(Error::Io)?;

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
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let crls = rustls_pemfile::crls(&mut reader)
        .collect::<std::result::Result<Vec<CertificateRevocationListDer<'static>>, _>>()
        .map_err(Error::Io)?;

    if !crls.is_empty() {
        return Ok(crls);
    }

    Ok(vec![CertificateRevocationListDer::from(std::fs::read(path)?)])
}

pub(crate) fn load_private_key_from_path(
    path: &Path,
) -> Result<rustls::pki_types::PrivateKeyDer<'static>> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    rustls_pemfile::private_key(&mut reader).map_err(Error::Io)?.ok_or_else(|| {
        Error::Server(format!(
            "server TLS private key file `{}` did not contain a supported PEM private key",
            path.display()
        ))
    })
}

fn build_ocsp_request_from_chain(
    certs: &[CertificateDer<'static>],
    path: &Path,
) -> Result<Vec<u8>> {
    let cert_id = build_ocsp_cert_id_from_chain(certs, path)?;
    let request = der_sequence([cert_id]);
    let request_list = der_sequence([request]);
    let tbs_request = der_sequence([request_list]);
    Ok(der_sequence([tbs_request]))
}

fn build_ocsp_cert_id_from_chain(
    certs: &[CertificateDer<'static>],
    path: &Path,
) -> Result<Vec<u8>> {
    if certs.len() < 2 {
        return Err(Error::Server(format!(
            "certificate `{}` requires a leaf and issuer certificate to build an OCSP request",
            path.display()
        )));
    }

    let (_, leaf) = X509Certificate::from_der(certs[0].as_ref()).map_err(|error| {
        Error::Server(format!(
            "failed to parse leaf certificate `{}` for OCSP request: {error}",
            path.display()
        ))
    })?;
    let (_, issuer) = X509Certificate::from_der(certs[1].as_ref()).map_err(|error| {
        Error::Server(format!(
            "failed to parse issuer certificate `{}` for OCSP request: {error}",
            path.display()
        ))
    })?;

    let issuer_name_hash = Sha1::digest(issuer.tbs_certificate.subject.as_raw());
    let issuer_key_hash = Sha1::digest(issuer.public_key().subject_public_key.data.as_ref());
    Ok(der_sequence([
        der_sequence([der_oid_sha1(), der_null()]),
        der_octet_string(issuer_name_hash.as_slice()),
        der_octet_string(issuer_key_hash.as_slice()),
        der_integer(leaf.raw_serial()),
    ]))
}

fn ocsp_responder_urls_from_cert(cert: &X509Certificate<'_>) -> Vec<String> {
    for extension in cert.iter_extensions() {
        if let ParsedExtension::AuthorityInfoAccess(aia) = extension.parsed_extension() {
            let mut urls = Vec::new();
            for access in &aia.accessdescs {
                if access.access_method.to_id_string() != "1.3.6.1.5.5.7.48.1" {
                    continue;
                }
                if let x509_parser::extensions::GeneralName::URI(uri) = &access.access_location {
                    let url = uri.to_string();
                    urls.push(url);
                }
            }
            if !urls.is_empty() {
                return urls;
            }
        }
    }
    Vec::new()
}

fn der_sequence<const N: usize>(elements: [Vec<u8>; N]) -> Vec<u8> {
    let payload = elements.into_iter().flatten().collect::<Vec<_>>();
    der_wrap(0x30, payload)
}

fn der_wrap(tag: u8, payload: Vec<u8>) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(1 + der_length(payload.len()).len() + payload.len());
    encoded.push(tag);
    encoded.extend(der_length(payload.len()));
    encoded.extend(payload);
    encoded
}

fn der_length(length: usize) -> Vec<u8> {
    if length < 0x80 {
        return vec![length as u8];
    }

    let bytes = length.to_be_bytes().into_iter().skip_while(|byte| *byte == 0).collect::<Vec<_>>();
    let mut encoded = Vec::with_capacity(bytes.len() + 1);
    encoded.push(0x80 | (bytes.len() as u8));
    encoded.extend(bytes);
    encoded
}

fn der_oid_sha1() -> Vec<u8> {
    vec![0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a]
}

fn der_null() -> Vec<u8> {
    vec![0x05, 0x00]
}

fn der_octet_string(bytes: &[u8]) -> Vec<u8> {
    der_wrap(0x04, bytes.to_vec())
}

fn der_integer(bytes: &[u8]) -> Vec<u8> {
    let mut value = bytes.iter().skip_while(|byte| **byte == 0).copied().collect::<Vec<_>>();
    if value.is_empty() {
        value.push(0);
    }
    if value.first().is_some_and(|byte| byte & 0x80 != 0) {
        value.insert(0, 0);
    }
    der_wrap(0x02, value)
}

fn validate_ocsp_response_time(
    path: &Path,
    response: &x509_ocsp::SingleResponse,
    now: SystemTime,
) -> Result<()> {
    let this_update = response.this_update.0.to_system_time();
    if this_update > now {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` is not yet valid (thisUpdate is in the future)",
            path.display()
        )));
    }

    if let Some(next_update) = response.next_update {
        let next_update = next_update.0.to_system_time();
        if next_update < now {
            return Err(Error::Server(format!(
                "OCSP response for certificate `{}` is expired (nextUpdate is in the past)",
                path.display()
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use der::asn1::{BitString, OctetString};
    use rcgen::{
        BasicConstraints, CertificateParams, CertifiedKey, DnType, IsCa, KeyPair, KeyUsagePurpose,
    };
    use spki::AlgorithmIdentifierOwned;
    use x509_ocsp::{
        BasicOcspResponse, CertId, CertStatus, OcspGeneralizedTime, OcspResponse, ResponderId,
        ResponseData, SingleResponse, Version,
    };

    use super::*;

    #[test]
    fn validate_ocsp_response_matches_current_certificate() {
        let temp_dir = temp_dir("rginx-ocsp-validate");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let leaf = generate_leaf_cert("localhost", &ca);
        let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
        let response = build_ocsp_response_for_certificate(&cert_path);

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
        std::fs::write(&ocsp_path, build_ocsp_response_for_certificate(&stale_cert_path))
            .expect("stale OCSP response should be written");

        let bundle =
            ServerCertificateBundle { cert_path, key_path, ocsp_staple_path: Some(ocsp_path) };
        let certified_key = load_certified_key_bundle(&bundle)
            .expect("certificate bundle should still load without reusing stale OCSP data");
        assert!(certified_key.ocsp.is_none(), "stale OCSP response should not be stapled");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    fn generate_ca_cert(common_name: &str) -> CertifiedKey {
        let mut params =
            CertificateParams::new(vec![common_name.to_string()]).expect("CA params should build");
        params.distinguished_name.push(DnType::CommonName, common_name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let key_pair = KeyPair::generate().expect("CA keypair should generate");
        let cert = params.self_signed(&key_pair).expect("CA certificate should self-sign");
        CertifiedKey { cert, key_pair }
    }

    fn generate_leaf_cert(common_name: &str, issuer: &CertifiedKey) -> CertifiedKey {
        let mut params = CertificateParams::new(vec![common_name.to_string()])
            .expect("leaf params should build");
        params.distinguished_name.push(DnType::CommonName, common_name);
        let key_pair = KeyPair::generate().expect("leaf keypair should generate");
        let cert = params
            .signed_by(&key_pair, &issuer.cert, &issuer.key_pair)
            .expect("leaf certificate should be signed");
        CertifiedKey { cert, key_pair }
    }

    fn write_cert_chain(
        temp_dir: &Path,
        name: &str,
        leaf: &CertifiedKey,
        ca: &CertifiedKey,
    ) -> PathBuf {
        let path = temp_dir.join(format!("{name}.crt"));
        std::fs::write(&path, format!("{}{}", leaf.cert.pem(), ca.cert.pem()))
            .expect("certificate chain should be written");
        path
    }

    fn write_private_key(temp_dir: &Path, name: &str, leaf: &CertifiedKey) -> PathBuf {
        let path = temp_dir.join(format!("{name}.key"));
        std::fs::write(&path, leaf.key_pair.serialize_pem())
            .expect("private key should be written");
        path
    }

    fn build_ocsp_response_for_certificate(cert_path: &Path) -> Vec<u8> {
        build_ocsp_response_for_certificate_with_offsets(
            cert_path,
            TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
            TimeOffset::After(Duration::from_secs(24 * 60 * 60)),
        )
    }

    fn build_ocsp_response_for_certificate_with_offsets(
        cert_path: &Path,
        this_update_offset: TimeOffset,
        next_update_offset: TimeOffset,
    ) -> Vec<u8> {
        let certs =
            load_certificate_chain_from_path(cert_path).expect("certificate chain should load");
        let cert_id = CertId::from_der(
            &build_ocsp_cert_id_from_chain(&certs, cert_path).expect("CertId should build"),
        )
        .expect("CertId should decode");
        let now = SystemTime::now();
        let this_update = ocsp_time_with_offset(now, this_update_offset);
        let next_update = ocsp_time_with_offset(now, next_update_offset);
        let basic = BasicOcspResponse {
            tbs_response_data: ResponseData {
                version: Version::V1,
                responder_id: ResponderId::ByKey(
                    OctetString::new(vec![1; 20]).expect("responder key hash should encode"),
                ),
                produced_at: this_update,
                responses: vec![SingleResponse {
                    cert_id,
                    cert_status: CertStatus::good(),
                    this_update,
                    next_update: Some(next_update),
                    single_extensions: None,
                }],
                response_extensions: None,
            },
            signature_algorithm: AlgorithmIdentifierOwned::from_der(&[
                0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b, 0x05,
                0x00,
            ])
            .expect("signature algorithm should decode"),
            signature: BitString::from_bytes(&[0x01]).expect("signature should encode"),
            certs: None,
        };
        OcspResponse::successful(basic)
            .expect("OCSP response should build")
            .to_der()
            .expect("OCSP response should encode")
    }

    enum TimeOffset {
        Before(Duration),
        After(Duration),
    }

    fn ocsp_time_with_offset(base: SystemTime, offset: TimeOffset) -> OcspGeneralizedTime {
        let time = match offset {
            TimeOffset::Before(duration) => {
                base.checked_sub(duration).expect("time offset should stay after unix epoch")
            }
            TimeOffset::After(duration) => base + duration,
        };
        OcspGeneralizedTime::try_from(time).expect("OCSP test time should be encodable")
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!("{prefix}-{unique}-{id}"))
    }
}
