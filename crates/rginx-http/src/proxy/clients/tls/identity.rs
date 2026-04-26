use std::path::Path;

use rginx_core::Error;
use rustls::RootCertStore;
use rustls::pki_types::{
    CertificateDer, PrivateKeyDer,
    pem::{Error as PemError, PemObject},
};
use rustls_native_certs::load_native_certs;

pub(crate) fn load_custom_ca_store(path: &Path) -> Result<RootCertStore, Error> {
    let certs = load_certificate_chain(path)?;
    let mut roots = RootCertStore::empty();
    let (added, _ignored) = roots.add_parsable_certificates(certs);
    if added == 0 || roots.is_empty() {
        return Err(Error::Server(format!(
            "no valid CA certificates were loaded from `{}`",
            path.display()
        )));
    }

    Ok(roots)
}

pub(super) fn load_native_root_store() -> Result<RootCertStore, Error> {
    let result = load_native_certs();
    if result.certs.is_empty() && !result.errors.is_empty() {
        return Err(Error::Server(format!("failed to load native TLS roots: {:?}", result.errors)));
    }

    let mut roots = RootCertStore::empty();
    let (added, _ignored) = roots.add_parsable_certificates(result.certs);
    if added == 0 || roots.is_empty() {
        return Err(Error::Server("no valid native TLS roots were loaded".to_string()));
    }
    Ok(roots)
}

pub(super) fn load_certificate_chain(path: &Path) -> Result<Vec<CertificateDer<'static>>, Error> {
    let certs = CertificateDer::pem_file_iter(path)
        .map_err(|error| map_pem_error(path, "certificates", error))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| map_pem_error(path, "certificates", error))?;

    if certs.is_empty() {
        let der = std::fs::read(path)?;
        return Ok(vec![CertificateDer::from(der)]);
    }

    Ok(certs)
}

pub(super) fn load_private_key(
    path: &Path,
) -> Result<rustls::pki_types::PrivateKeyDer<'static>, Error> {
    match PrivateKeyDer::from_pem_file(path) {
        Ok(key) => Ok(key),
        Err(PemError::NoItemsFound) => Err(Error::Server(format!(
            "private key file `{}` did not contain a supported PEM private key",
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
