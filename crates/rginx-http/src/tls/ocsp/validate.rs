use super::*;

pub(crate) fn validate_ocsp_response_for_certificate(
    path: &Path,
    response_der: &[u8],
) -> Result<()> {
    validate_ocsp_response_for_certificate_with_options(
        path,
        response_der,
        None,
        OcspNonceMode::Disabled,
        OcspResponderPolicy::IssuerOrDelegated,
    )
}

pub(crate) fn validate_ocsp_response_for_certificate_with_options(
    path: &Path,
    response_der: &[u8],
    expected_nonce: Option<&[u8]>,
    nonce_mode: OcspNonceMode,
    responder_policy: OcspResponderPolicy,
) -> Result<()> {
    let certs = load_certificate_chain_from_path(path)?;
    let expected_cert_id = build_rasn_ocsp_cert_id_from_chain(&certs, path)?;
    let (_, issuer) = parse_leaf_and_issuer_certificates(&certs, path, "OCSP response validation")?;
    let now = SystemTime::now();

    let response: RasnOcspResponse = rasn::der::decode(response_der).map_err(|error| {
        Error::Server(format!(
            "failed to parse OCSP response for certificate `{}`: {error}",
            path.display()
        ))
    })?;
    if response.status != RasnOcspResponseStatus::Successful {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` is not successful: {:?}",
            path.display(),
            response.status
        )));
    }

    let response_bytes = response.bytes.ok_or_else(|| {
        Error::Server(format!(
            "OCSP response for certificate `{}` is missing response_bytes",
            path.display()
        ))
    })?;
    if response_bytes.r#type != basic_ocsp_response_type_oid() {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` uses unsupported response type `{}`",
            path.display(),
            response_bytes.r#type
        )));
    }

    let basic_response: RasnBasicOcspResponse = rasn::der::decode(response_bytes.response.as_ref())
        .map_err(|error| {
            Error::Server(format!(
                "failed to parse basic OCSP response for certificate `{}`: {error}",
                path.display()
            ))
        })?;
    validate_basic_ocsp_response(
        path,
        &basic_response,
        &issuer,
        expected_nonce,
        nonce_mode,
        responder_policy,
        now,
    )?;

    let matched_response = matching_single_response_for_cert(
        path,
        &basic_response.tbs_response_data,
        &expected_cert_id,
    )?;
    validate_ocsp_cert_status(path, matched_response)?;
    validate_ocsp_response_time(path, &basic_response.tbs_response_data, matched_response, now)
}

pub(super) fn validate_basic_ocsp_response(
    path: &Path,
    basic_response: &RasnBasicOcspResponse,
    issuer: &RasnCertificate,
    expected_nonce: Option<&[u8]>,
    nonce_mode: OcspNonceMode,
    responder_policy: OcspResponderPolicy,
    now: SystemTime,
) -> Result<()> {
    validate_ocsp_produced_at(path, &basic_response.tbs_response_data, now)?;
    validate_ocsp_nonce(
        path,
        basic_response.tbs_response_data.response_extensions.as_ref(),
        expected_nonce,
        nonce_mode,
    )?;
    validate_basic_ocsp_response_signature(path, basic_response, issuer, responder_policy)
}

pub(super) fn validate_ocsp_nonce(
    path: &Path,
    response_extensions: Option<&rasn_pkix::Extensions>,
    expected_nonce: Option<&[u8]>,
    nonce_mode: OcspNonceMode,
) -> Result<()> {
    if nonce_mode != OcspNonceMode::Disabled && expected_nonce.is_none() {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` could not validate nonce because the request nonce was unavailable",
            path.display()
        )));
    }

    let Some(expected_nonce) = expected_nonce else {
        return Ok(());
    };

    let response_nonce = extract_ocsp_nonce(path, response_extensions)?;
    match nonce_mode {
        OcspNonceMode::Disabled => Ok(()),
        OcspNonceMode::Preferred => {
            if let Some(response_nonce) = response_nonce
                && response_nonce.as_slice() != expected_nonce
            {
                return Err(Error::Server(format!(
                    "OCSP response for certificate `{}` returned a mismatched nonce",
                    path.display()
                )));
            }
            Ok(())
        }
        OcspNonceMode::Required => {
            let response_nonce = response_nonce.ok_or_else(|| {
                Error::Server(format!(
                    "OCSP response for certificate `{}` did not echo the required nonce",
                    path.display()
                ))
            })?;
            if response_nonce.as_slice() != expected_nonce {
                return Err(Error::Server(format!(
                    "OCSP response for certificate `{}` returned a mismatched nonce",
                    path.display()
                )));
            }
            Ok(())
        }
    }
}

pub(super) fn validate_ocsp_produced_at(
    path: &Path,
    response_data: &RasnResponseData,
    now: SystemTime,
) -> Result<()> {
    let now = generalized_time_from_system_time(now);
    if response_data.produced_at > now {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` is not yet valid (producedAt is in the future)",
            path.display()
        )));
    }
    Ok(())
}

pub(super) fn matching_single_response_for_cert<'a>(
    path: &Path,
    response_data: &'a RasnResponseData,
    expected_cert_id: &RasnCertId,
) -> Result<&'a RasnSingleResponse> {
    let mut matches =
        response_data.responses.iter().filter(|response| response.cert_id == *expected_cert_id);
    let response = matches.next().ok_or_else(|| {
        Error::Server(format!(
            "OCSP response for certificate `{}` does not contain a matching SingleResponse",
            path.display()
        ))
    })?;
    if matches.next().is_some() {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` contains multiple matching SingleResponses",
            path.display()
        )));
    }
    Ok(response)
}

pub(super) fn validate_ocsp_cert_status(path: &Path, response: &RasnSingleResponse) -> Result<()> {
    match &response.cert_status {
        RasnCertStatus::Good => Ok(()),
        RasnCertStatus::Revoked(_) => Err(Error::Server(format!(
            "OCSP response for certificate `{}` reports the certificate as revoked",
            path.display()
        ))),
        RasnCertStatus::Unknown(_) => Err(Error::Server(format!(
            "OCSP response for certificate `{}` reports an unknown certificate status",
            path.display()
        ))),
    }
}

pub(super) fn validate_ocsp_response_time(
    path: &Path,
    response_data: &RasnResponseData,
    response: &RasnSingleResponse,
    now: SystemTime,
) -> Result<()> {
    let now = generalized_time_from_system_time(now);
    if response_data.produced_at < response.this_update {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` is inconsistent (producedAt precedes thisUpdate)",
            path.display()
        )));
    }
    let this_update = &response.this_update;
    if this_update > &now {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` is not yet valid (thisUpdate is in the future)",
            path.display()
        )));
    }

    if let Some(next_update) = &response.next_update
        && next_update < &now
    {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` is expired (nextUpdate is in the past)",
            path.display()
        )));
    }

    Ok(())
}
