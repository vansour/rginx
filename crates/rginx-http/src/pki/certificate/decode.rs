use std::path::Path;

use rasn_pkix::Certificate;
use rustls::pki_types::{CertificateDer, pem::PemObject};

pub(super) fn decode_certificate(bytes: &[u8]) -> Option<Certificate> {
    rasn::der::decode(bytes).ok()
}

pub(super) fn load_certificate_chain_der(path: &Path) -> std::io::Result<Vec<Vec<u8>>> {
    let certs = CertificateDer::pem_file_iter(path)
        .map_err(pem_error_to_io_error)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(pem_error_to_io_error)?;
    if !certs.is_empty() {
        return Ok(certs.into_iter().map(|cert| cert.as_ref().to_vec()).collect());
    }
    Ok(vec![std::fs::read(path)?])
}

fn pem_error_to_io_error(error: rustls::pki_types::pem::Error) -> std::io::Error {
    match error {
        rustls::pki_types::pem::Error::Io(error) => error,
        other => std::io::Error::new(std::io::ErrorKind::InvalidData, other),
    }
}
