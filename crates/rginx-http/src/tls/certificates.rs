use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use rginx_core::{Error, Result, ServerCertificateBundle, ServerTls, VirtualHostTls};
use rustls::RootCertStore;
use rustls::pki_types::{CertificateDer, CertificateRevocationListDer};

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
