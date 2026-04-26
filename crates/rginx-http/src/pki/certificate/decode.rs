use std::path::Path;

use rasn_pkix::Certificate;
use rustls::pki_types::{CertificateDer, pem::PemObject};

pub(super) fn decode_certificate(bytes: &[u8]) -> Option<Certificate> {
    rasn::der::decode(bytes).ok()
}

pub(super) fn load_certificate_chain_der(path: &Path) -> std::io::Result<Vec<Vec<u8>>> {
    let bytes = std::fs::read(path)?;
    if !looks_like_pem(bytes.as_ref()) {
        return Ok(vec![bytes]);
    }

    let certs = CertificateDer::pem_slice_iter(bytes.as_ref())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(pem_error_to_io_error)?;
    if !certs.is_empty() {
        return Ok(certs.into_iter().map(|cert| cert.as_ref().to_vec()).collect());
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "PEM input does not contain any CERTIFICATE blocks",
    ))
}

fn pem_error_to_io_error(error: rustls::pki_types::pem::Error) -> std::io::Error {
    match error {
        rustls::pki_types::pem::Error::Io(error) => error,
        other => std::io::Error::new(std::io::ErrorKind::InvalidData, other),
    }
}

fn looks_like_pem(bytes: &[u8]) -> bool {
    bytes.windows("-----BEGIN ".len()).any(|window| window == b"-----BEGIN ")
}
