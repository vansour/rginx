use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use rginx_core::{Error, Result, ServerCertificateBundle, ServerTls, VirtualHostTls};
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, CertificateRevocationListDer};
use sha1::{Digest, Sha1};
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
        if !ocsp.is_empty() {
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
    let cert_id = der_sequence([
        der_sequence([der_oid_sha1(), der_null()]),
        der_octet_string(issuer_name_hash.as_slice()),
        der_octet_string(issuer_key_hash.as_slice()),
        der_integer(leaf.raw_serial()),
    ]);
    let request = der_sequence([cert_id]);
    let request_list = der_sequence([request]);
    let tbs_request = der_sequence([request_list]);
    Ok(der_sequence([tbs_request]))
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
