use std::path::Path;

use rasn_pkix::CertificateList;
use rginx_core::{Error, Result};

pub(crate) fn validate_der_certificate_revocation_list(path: &Path, der: &[u8]) -> Result<()> {
    let (_crl, remaining) =
        rasn::der::decode_with_remainder::<CertificateList>(der).map_err(|error| {
            Error::Server(format!(
                "failed to parse certificate revocation list `{}` as PEM or DER CRL: {error}",
                path.display()
            ))
        })?;
    if !remaining.is_empty() {
        return Err(Error::Server(format!(
            "certificate revocation list `{}` contains trailing data after the DER CRL payload",
            path.display()
        )));
    }
    Ok(())
}
