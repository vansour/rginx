use super::*;

pub(super) fn ocsp_nonce_oid() -> ObjectIdentifier {
    ObjectIdentifier::new(vec![1, 3, 6, 1, 5, 5, 7, 48, 1, 2])
        .expect("OCSP nonce OID should be valid")
}

pub(super) fn build_request_nonce(
    path: &Path,
    nonce_mode: OcspNonceMode,
) -> Result<Option<Vec<u8>>> {
    if nonce_mode == OcspNonceMode::Disabled {
        return Ok(None);
    }

    let mut nonce = vec![0_u8; 16];
    rustls::crypto::aws_lc_rs::default_provider().secure_random.fill(&mut nonce).map_err(
        |error| {
            Error::Server(format!(
                "failed to generate OCSP nonce for certificate `{}`: {:?}",
                path.display(),
                error
            ))
        },
    )?;
    Ok(Some(nonce))
}

pub(super) fn build_ocsp_nonce_extension(nonce: &[u8]) -> Result<RasnExtension> {
    let extn_value = rasn::der::encode(&OctetString::from_slice(nonce)).map_err(|error| {
        Error::Server(format!("failed to encode OCSP nonce extension value: {error}"))
    })?;
    Ok(RasnExtension {
        extn_id: ocsp_nonce_oid(),
        critical: false,
        extn_value: OctetString::from_slice(&extn_value),
    })
}

pub(super) fn extract_ocsp_nonce(
    path: &Path,
    extensions: Option<&rasn_pkix::Extensions>,
) -> Result<Option<Vec<u8>>> {
    let Some(extensions) = extensions else {
        return Ok(None);
    };

    let mut nonce = None;
    for extension in extensions.iter() {
        if extension.extn_id != ocsp_nonce_oid() {
            continue;
        }
        if nonce.is_some() {
            return Err(Error::Server(format!(
                "OCSP response for certificate `{}` contains duplicate nonce extensions",
                path.display()
            )));
        }
        let parsed_nonce: OctetString =
            rasn::der::decode(extension.extn_value.as_ref()).map_err(|error| {
                Error::Server(format!(
                    "failed to parse OCSP nonce extension for certificate `{}`: {error}",
                    path.display()
                ))
            })?;
        nonce = Some(parsed_nonce.to_vec());
    }

    Ok(nonce)
}
