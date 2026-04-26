use super::*;

pub(super) fn validate_basic_ocsp_response_signature(
    path: &Path,
    basic_response: &RasnBasicOcspResponse,
    issuer: &RasnCertificate,
    responder_policy: OcspResponderPolicy,
) -> Result<()> {
    let tbs_response_der =
        rasn::der::encode(&basic_response.tbs_response_data).map_err(|error| {
            Error::Server(format!(
                "failed to encode OCSP response data for certificate `{}`: {error}",
                path.display()
            ))
        })?;
    let signature_algorithm_der = algorithm_identifier_value_bytes(
        path,
        &basic_response.signature_algorithm,
        "failed to encode OCSP signature algorithm",
    )?;
    let signature_value = signature_bytes(path, &basic_response.signature)?;

    let responder_id = &basic_response.tbs_response_data.responder_id;
    let mut last_error = None;

    if responder_id_matches_certificate(path, responder_id, issuer)? {
        match validate_ocsp_signer_candidate(
            path,
            issuer,
            issuer,
            &tbs_response_der,
            &signature_algorithm_der,
            signature_value,
            responder_policy,
        ) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }

    for cert in basic_response.certs.iter().flatten() {
        let signer = cert.clone();
        if !responder_id_matches_certificate(path, responder_id, &signer)? {
            continue;
        }
        match validate_ocsp_signer_candidate(
            path,
            &signer,
            issuer,
            &tbs_response_der,
            &signature_algorithm_der,
            signature_value,
            responder_policy,
        ) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }

    Err(last_error.unwrap_or_else(|| {
        Error::Server(format!(
            "OCSP response for certificate `{}` does not contain an authorized responder certificate matching responderId",
            path.display()
        ))
    }))
}

pub(super) fn responder_id_matches_certificate(
    path: &Path,
    responder_id: &RasnResponderId,
    cert: &RasnCertificate,
) -> Result<bool> {
    match responder_id {
        RasnResponderId::ByName(name) => {
            let encoded_name = rasn::der::encode(name).map_err(|error| {
                Error::Server(format!(
                    "failed to encode OCSP responderId name for certificate `{}`: {error}",
                    path.display()
                ))
            })?;
            let subject_der =
                rasn::der::encode(&cert.tbs_certificate.subject).map_err(|error| {
                    Error::Server(format!(
                        "failed to encode responder certificate subject for `{}`: {error}",
                        path.display()
                    ))
                })?;
            Ok(encoded_name.as_slice() == subject_der.as_slice())
        }
        RasnResponderId::ByKey(key_hash) => {
            let responder_key_hash = Sha1::digest(subject_public_key_bytes(cert));
            Ok(responder_key_hash.as_slice() == key_hash.as_ref())
        }
    }
}

pub(super) fn validate_ocsp_signer_candidate(
    path: &Path,
    signer: &RasnCertificate,
    issuer: &RasnCertificate,
    tbs_response_der: &[u8],
    signature_algorithm_der: &[u8],
    signature_value: &[u8],
    responder_policy: OcspResponderPolicy,
) -> Result<()> {
    authorize_ocsp_signer(path, signer, issuer, responder_policy)?;
    verify_signature_with_webpki(
        path,
        signer,
        signature_algorithm_der,
        signature_value,
        tbs_response_der,
        "OCSP response for certificate",
        "has an invalid responder signature",
    )
}

pub(super) fn authorize_ocsp_signer(
    path: &Path,
    signer: &RasnCertificate,
    issuer: &RasnCertificate,
    responder_policy: OcspResponderPolicy,
) -> Result<()> {
    if signer == issuer {
        return Ok(());
    }

    if responder_policy == OcspResponderPolicy::IssuerOnly {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` uses a delegated responder certificate but policy is issuer_only",
            path.display()
        )));
    }

    if signer.tbs_certificate.issuer != issuer.tbs_certificate.subject {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` uses a responder certificate that is not issued by the certificate issuer",
            path.display()
        )));
    }
    verify_certificate_signature_with_webpki(path, signer, issuer)?;
    if !certificate_valid_now(signer, SystemTime::now()) {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` uses an expired or not-yet-valid responder certificate",
            path.display()
        )));
    }

    let Some(extended_key_usage) = certificate_extended_key_usage(signer)? else {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` uses a delegated responder certificate without an OCSP signing extended key usage",
            path.display()
        )));
    };
    if !extended_key_usage.iter().any(|oid| oid.to_string() == "1.3.6.1.5.5.7.3.9") {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` uses a delegated responder certificate without an OCSP signing extended key usage",
            path.display()
        )));
    }

    if let Some(key_usage) = certificate_key_usage(signer)?
        && !bit_string_flag(&key_usage, 0)
    {
        return Err(Error::Server(format!(
            "OCSP response for certificate `{}` uses a delegated responder certificate without digitalSignature key usage",
            path.display()
        )));
    }

    Ok(())
}

pub(super) fn verify_certificate_signature_with_webpki(
    path: &Path,
    signer: &RasnCertificate,
    issuer: &RasnCertificate,
) -> Result<()> {
    let tbs_certificate_der = rasn::der::encode(&signer.tbs_certificate).map_err(|error| {
        Error::Server(format!(
            "failed to encode responder certificate TBS for `{}` during issuer signature verification: {error}",
            path.display()
        ))
    })?;
    let signature_algorithm_der = algorithm_identifier_value_bytes(
        path,
        &signer.signature_algorithm,
        "failed to encode responder certificate signature algorithm",
    )?;
    let signature_value = signature_bytes(path, &signer.signature_value)?;

    verify_signature_with_webpki(
        path,
        issuer,
        &signature_algorithm_der,
        signature_value,
        &tbs_certificate_der,
        "OCSP response for certificate",
        "uses a responder certificate with an invalid issuer signature",
    )
}

pub(super) fn verify_signature_with_webpki(
    path: &Path,
    certificate: &RasnCertificate,
    signature_algorithm_der: &[u8],
    signature_value: &[u8],
    message: &[u8],
    scope_prefix: &str,
    error_suffix: &str,
) -> Result<()> {
    let certificate_der = rasn::der::encode(certificate).map_err(|error| {
        Error::Server(format!(
            "failed to encode certificate `{}` for signature verification: {error}",
            path.display()
        ))
    })?;
    let certificate_der = CertificateDer::from(certificate_der);
    let certificate = EndEntityCert::try_from(&certificate_der).map_err(|error| {
        Error::Server(format!(
            "failed to parse certificate `{}` for signature verification: {error}",
            path.display()
        ))
    })?;

    let candidates = ALL_VERIFICATION_ALGS
        .iter()
        .copied()
        .filter(|algorithm| algorithm.signature_alg_id().as_ref() == signature_algorithm_der)
        .collect::<Vec<_>>();

    if candidates.is_empty() {
        return Err(Error::Server(format!(
            "{scope_prefix} `{}` uses unsupported signature algorithm `{}`",
            path.display(),
            hex_string(signature_algorithm_der)
        )));
    }

    let mut last_public_key_mismatch = None;
    for algorithm in candidates {
        match certificate.verify_signature(algorithm, message, signature_value) {
            Ok(()) => return Ok(()),
            Err(error @ webpki::Error::UnsupportedSignatureAlgorithmForPublicKeyContext(_)) => {
                last_public_key_mismatch = Some(error);
            }
            Err(error) => {
                return Err(Error::Server(format!(
                    "{scope_prefix} `{}` {error_suffix}: {error}",
                    path.display()
                )));
            }
        }
    }

    let error =
        last_public_key_mismatch.expect("candidates should produce an error when none verify");
    Err(Error::Server(format!("{scope_prefix} `{}` {error_suffix}: {error}", path.display())))
}
