use super::*;

pub(crate) fn build_ocsp_request_for_certificate(path: &Path) -> Result<Vec<u8>> {
    build_ocsp_request_for_certificate_with_options(path, OcspNonceMode::Disabled)
        .map(|(request, _nonce)| request)
}

pub(crate) fn build_ocsp_request_for_certificate_with_options(
    path: &Path,
    nonce_mode: OcspNonceMode,
) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
    let certs = load_certificate_chain_from_path(path)?;
    build_ocsp_request_from_chain(&certs, path, nonce_mode)
}

fn build_ocsp_request_from_chain(
    certs: &[CertificateDer<'static>],
    path: &Path,
    nonce_mode: OcspNonceMode,
) -> Result<(Vec<u8>, Option<Vec<u8>>)> {
    let cert_id = build_rasn_ocsp_cert_id_from_chain(certs, path)?;
    let request_nonce = build_request_nonce(path, nonce_mode)?;
    let request = RasnOcspRequest {
        tbs_request: TbsRequest {
            version: Integer::from(0),
            requestor_name: None,
            request_list: vec![Request { req_cert: cert_id, single_request_extensions: None }],
            request_extensions: request_nonce
                .as_deref()
                .map(build_ocsp_nonce_extension)
                .transpose()?
                .map(|extension| vec![extension].into()),
        },
        optional_signature: None,
    };
    let request = rasn::der::encode(&request).map_err(|error| {
        Error::Server(format!(
            "failed to encode OCSP request for certificate `{}`: {error}",
            path.display()
        ))
    })?;
    Ok((request, request_nonce))
}
