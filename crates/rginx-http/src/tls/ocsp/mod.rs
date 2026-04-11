use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use rasn::types::{GeneralizedTime, Integer, ObjectIdentifier, OctetString};
use rasn_ocsp::{
    BasicOcspResponse as RasnBasicOcspResponse, CertId as RasnCertId, CertStatus as RasnCertStatus,
    OcspRequest as RasnOcspRequest, OcspResponse as RasnOcspResponse,
    OcspResponseStatus as RasnOcspResponseStatus, Request, ResponderId as RasnResponderId,
    ResponseData as RasnResponseData, SingleResponse as RasnSingleResponse, TbsRequest,
};
use rasn_pkix::{
    AlgorithmIdentifier, AuthorityInfoAccessSyntax, Certificate as RasnCertificate,
    ExtKeyUsageSyntax, Extension as RasnExtension, GeneralName as RasnGeneralName,
    KeyUsage as RasnKeyUsage, Time as RasnTime, algorithms::ID_SHA1,
};
use rginx_core::{Error, OcspNonceMode, OcspResponderPolicy, Result};
use rustls::pki_types::CertificateDer;
use sha1::{Digest, Sha1};
use webpki::{ALL_VERIFICATION_ALGS, EndEntityCert};

use super::certificates::load_certificate_chain_from_path;

#[cfg(test)]
use super::certificates::load_certified_key_bundle;

pub(crate) fn ocsp_responder_urls_for_certificate(path: &Path) -> Result<Vec<String>> {
    let certs = load_certificate_chain_from_path(path)?;
    let Some(leaf) = certs.first() else {
        return Ok(Vec::new());
    };

    let cert: RasnCertificate = rasn::der::decode(leaf.as_ref()).map_err(|error| {
        Error::Server(format!(
            "failed to parse X.509 certificate `{}` for OCSP responder discovery: {error}",
            path.display()
        ))
    })?;
    Ok(ocsp_responder_urls_from_cert(&cert))
}

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

fn parse_leaf_and_issuer_certificates(
    certs: &[CertificateDer<'static>],
    path: &Path,
    context: &str,
) -> Result<(RasnCertificate, RasnCertificate)> {
    if certs.len() < 2 {
        return Err(Error::Server(format!(
            "certificate `{}` requires a leaf and issuer certificate to build an OCSP request",
            path.display()
        )));
    }

    let leaf: RasnCertificate = rasn::der::decode(certs[0].as_ref()).map_err(|error| {
        Error::Server(format!(
            "failed to parse leaf certificate `{}` for {context}: {error}",
            path.display()
        ))
    })?;
    let issuer: RasnCertificate = rasn::der::decode(certs[1].as_ref()).map_err(|error| {
        Error::Server(format!(
            "failed to parse issuer certificate `{}` for {context}: {error}",
            path.display()
        ))
    })?;
    Ok((leaf, issuer))
}

fn build_rasn_ocsp_cert_id_from_chain(
    certs: &[CertificateDer<'static>],
    path: &Path,
) -> Result<RasnCertId> {
    let (leaf, issuer) = parse_leaf_and_issuer_certificates(certs, path, "OCSP request")?;

    let issuer_name_der = rasn::der::encode(&issuer.tbs_certificate.subject).map_err(|error| {
        Error::Server(format!(
            "failed to encode issuer subject name for certificate `{}` while building OCSP request: {error}",
            path.display()
        ))
    })?;
    let issuer_name_hash = Sha1::digest(&issuer_name_der);
    let issuer_key_hash = Sha1::digest(subject_public_key_bytes(&issuer));
    let serial_number = leaf.tbs_certificate.serial_number.clone();

    Ok(RasnCertId {
        hash_algorithm: AlgorithmIdentifier { algorithm: ID_SHA1.to_owned(), parameters: None },
        issuer_name_hash: OctetString::from(issuer_name_hash.as_slice()),
        issuer_key_hash: OctetString::from(issuer_key_hash.as_slice()),
        serial_number,
    })
}

fn ocsp_responder_urls_from_cert(cert: &RasnCertificate) -> Vec<String> {
    const OID_AUTHORITY_INFO_ACCESS: &str = "1.3.6.1.5.5.7.1.1";
    const OID_OCSP_ACCESS_METHOD: &str = "1.3.6.1.5.5.7.48.1";

    let Some(extensions) = cert.tbs_certificate.extensions.as_ref() else {
        return Vec::new();
    };

    for extension in extensions.iter() {
        if extension.extn_id.to_string() != OID_AUTHORITY_INFO_ACCESS {
            continue;
        }
        let Ok(aia) = rasn::der::decode::<AuthorityInfoAccessSyntax>(extension.extn_value.as_ref())
        else {
            continue;
        };
        let mut urls = Vec::new();
        for access in aia {
            if access.access_method.to_string() != OID_OCSP_ACCESS_METHOD {
                continue;
            }
            if let RasnGeneralName::Uri(uri) = access.access_location {
                urls.push(uri.to_string());
            }
        }
        if !urls.is_empty() {
            return urls;
        }
    }

    Vec::new()
}

fn basic_ocsp_response_type_oid() -> ObjectIdentifier {
    ObjectIdentifier::new(vec![1, 3, 6, 1, 5, 5, 7, 48, 1, 1])
        .expect("basic OCSP response type OID should be valid")
}

fn ocsp_nonce_oid() -> ObjectIdentifier {
    ObjectIdentifier::new(vec![1, 3, 6, 1, 5, 5, 7, 48, 1, 2])
        .expect("OCSP nonce OID should be valid")
}

fn generalized_time_from_system_time(time: SystemTime) -> GeneralizedTime {
    let utc = DateTime::<Utc>::from(time);
    utc.fixed_offset()
}

fn build_request_nonce(path: &Path, nonce_mode: OcspNonceMode) -> Result<Option<Vec<u8>>> {
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

fn build_ocsp_nonce_extension(nonce: &[u8]) -> Result<RasnExtension> {
    let extn_value = rasn::der::encode(&OctetString::from_slice(nonce)).map_err(|error| {
        Error::Server(format!("failed to encode OCSP nonce extension value: {error}"))
    })?;
    Ok(RasnExtension {
        extn_id: ocsp_nonce_oid(),
        critical: false,
        extn_value: OctetString::from_slice(&extn_value),
    })
}

fn extract_ocsp_nonce(
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

fn validate_basic_ocsp_response(
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

fn validate_ocsp_nonce(
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

fn validate_ocsp_produced_at(
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

fn matching_single_response_for_cert<'a>(
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

fn validate_ocsp_cert_status(path: &Path, response: &RasnSingleResponse) -> Result<()> {
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

fn validate_basic_ocsp_response_signature(
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

fn responder_id_matches_certificate(
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

fn validate_ocsp_signer_candidate(
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

fn authorize_ocsp_signer(
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

fn subject_public_key_bytes(cert: &RasnCertificate) -> &[u8] {
    cert.tbs_certificate.subject_public_key_info.subject_public_key.as_raw_slice()
}

fn certificate_extensions(cert: &RasnCertificate) -> Option<&rasn_pkix::Extensions> {
    cert.tbs_certificate.extensions.as_ref()
}

fn find_certificate_extension<'a>(
    cert: &'a RasnCertificate,
    oid: &str,
) -> Option<&'a rasn_pkix::Extension> {
    certificate_extensions(cert)?.iter().find(|extension| extension.extn_id.to_string() == oid)
}

fn certificate_extended_key_usage(cert: &RasnCertificate) -> Result<Option<ExtKeyUsageSyntax>> {
    let Some(extension) = find_certificate_extension(cert, "2.5.29.37") else {
        return Ok(None);
    };
    rasn::der::decode(extension.extn_value.as_ref()).map(Some).map_err(|error| {
        Error::Server(format!(
            "failed to inspect responder certificate extended key usage for `{}`: {error}",
            "<embedded>"
        ))
    })
}

fn certificate_key_usage(cert: &RasnCertificate) -> Result<Option<RasnKeyUsage>> {
    let Some(extension) = find_certificate_extension(cert, "2.5.29.15") else {
        return Ok(None);
    };
    rasn::der::decode(extension.extn_value.as_ref()).map(Some).map_err(|error| {
        Error::Server(format!(
            "failed to inspect responder certificate key usage for `{}`: {error}",
            "<embedded>"
        ))
    })
}

fn bit_string_flag(value: &RasnKeyUsage, index: usize) -> bool {
    value.get(index).map(|bit| *bit).unwrap_or(false)
}

fn certificate_valid_now(cert: &RasnCertificate, now: SystemTime) -> bool {
    let now = DateTime::<Utc>::from(now).timestamp();
    let Some(not_before) = rasn_time_to_unix_seconds(cert.tbs_certificate.validity.not_before)
    else {
        return false;
    };
    let Some(not_after) = rasn_time_to_unix_seconds(cert.tbs_certificate.validity.not_after) else {
        return false;
    };
    not_before <= now && now <= not_after
}

fn rasn_time_to_unix_seconds(time: RasnTime) -> Option<i64> {
    Some(match time {
        RasnTime::Utc(value) => value.timestamp(),
        RasnTime::General(value) => value.timestamp(),
    })
}

fn verify_certificate_signature_with_webpki(
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

fn verify_signature_with_webpki(
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

fn hex_string(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
}

fn algorithm_identifier_value_bytes(
    path: &Path,
    algorithm: &AlgorithmIdentifier,
    context: &str,
) -> Result<Vec<u8>> {
    // webpki::SignatureVerificationAlgorithm compares against the raw
    // AlgorithmIdentifier value bytes (SEQUENCE contents), not the outer DER
    // wrapping of the SEQUENCE itself.
    let mut value = rasn::der::encode(&algorithm.algorithm).map_err(|error| {
        Error::Server(format!("{context} for certificate `{}`: {error}", path.display()))
    })?;
    if let Some(parameters) = algorithm.parameters.as_ref() {
        value.extend_from_slice(parameters.as_ref());
    }
    Ok(value)
}

fn signature_bytes<'a>(path: &Path, signature: &'a rasn::types::BitString) -> Result<&'a [u8]> {
    if signature.len() % 8 != 0 {
        return Err(Error::Server(format!(
            "failed to parse OCSP signature value for certificate `{}`: signature BIT STRING contains unused bits",
            path.display()
        )));
    }
    Ok(signature.as_raw_slice())
}

fn validate_ocsp_response_time(
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

    if let Some(next_update) = &response.next_update {
        if next_update < &now {
            return Err(Error::Server(format!(
                "OCSP response for certificate `{}` is expired (nextUpdate is in the past)",
                path.display()
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use rasn::types::{BitString, GeneralizedTime, Integer, OctetString};
    use rasn_ocsp::{
        BasicOcspResponse as RasnBasicOcspResponse, CertStatus as RasnCertStatus,
        OcspResponse as RasnOcspResponse, OcspResponseStatus as RasnOcspResponseStatus,
        ResponderId as RasnResponderId, ResponseBytes as RasnResponseBytes,
        ResponseData as RasnResponseData, SingleResponse as RasnSingleResponse,
    };
    use rcgen::{
        BasicConstraints, CertificateParams, CustomExtension, DnType, ExtendedKeyUsagePurpose,
        IsCa, Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, PKCS_ED25519,
        PKCS_RSA_SHA256, SigningKey,
    };
    use rginx_core::ServerCertificateBundle;

    use super::*;

    struct TestCertifiedKey {
        cert: rcgen::Certificate,
        signing_key: KeyPair,
        params: CertificateParams,
    }

    impl TestCertifiedKey {
        fn issuer(&self) -> Issuer<'_, &KeyPair> {
            Issuer::from_params(&self.params, &self.signing_key)
        }
    }

    #[test]
    fn validate_ocsp_response_matches_current_certificate() {
        let temp_dir = temp_dir("rginx-ocsp-validate");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let leaf = generate_leaf_cert("localhost", &ca);
        let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
        let response = build_ocsp_response_for_certificate(&cert_path, &ca);

        validate_ocsp_response_for_certificate(&cert_path, &response)
            .expect("OCSP response should match the current certificate");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn validate_ocsp_response_rejects_expired_response() {
        let temp_dir = temp_dir("rginx-ocsp-expired");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let leaf = generate_leaf_cert("localhost", &ca);
        let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
        let response = build_ocsp_response_for_certificate_with_offsets(
            &cert_path,
            &ca,
            TimeOffset::Before(Duration::from_secs(2 * 24 * 60 * 60)),
            TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
        );

        let error = validate_ocsp_response_for_certificate(&cert_path, &response)
            .expect_err("expired OCSP response should be rejected");
        assert!(error.to_string().contains("is expired"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn load_certified_key_bundle_ignores_stale_ocsp_cache() {
        let temp_dir = temp_dir("rginx-ocsp-stale-cache");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let current_leaf = generate_leaf_cert("localhost", &ca);
        let stale_leaf = generate_leaf_cert("localhost", &ca);
        let cert_path = write_cert_chain(&temp_dir, "current", &current_leaf, &ca);
        let key_path = write_private_key(&temp_dir, "current", &current_leaf);
        let stale_cert_path = write_cert_chain(&temp_dir, "stale", &stale_leaf, &ca);
        let ocsp_path = temp_dir.join("server.ocsp");
        std::fs::write(&ocsp_path, build_ocsp_response_for_certificate(&stale_cert_path, &ca))
            .expect("stale OCSP response should be written");

        let bundle = ServerCertificateBundle {
            cert_path,
            key_path,
            ocsp_staple_path: Some(ocsp_path),
            ocsp: rginx_core::OcspConfig::default(),
        };
        let certified_key = load_certified_key_bundle(&bundle)
            .expect("certificate bundle should still load without reusing stale OCSP data");
        assert!(certified_key.ocsp.is_none(), "stale OCSP response should not be stapled");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn validate_ocsp_response_rejects_future_produced_at() {
        let temp_dir = temp_dir("rginx-ocsp-produced-at-future");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let leaf = generate_leaf_cert("localhost", &ca);
        let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
        let response = build_ocsp_response_for_certificate_with_signer(
            &cert_path,
            TimeOffset::Before(Duration::from_secs(60)),
            Some(TimeOffset::After(Duration::from_secs(24 * 60 * 60))),
            TimeOffset::After(Duration::from_secs(60)),
            RasnCertStatus::Good,
            OcspResponseSigner::Issuer(&ca),
            None,
            false,
            false,
        );

        let error = validate_ocsp_response_for_certificate(&cert_path, &response)
            .expect_err("future producedAt should be rejected");
        assert!(error.to_string().contains("producedAt is in the future"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn validate_ocsp_response_rejects_unknown_certificate_status() {
        let temp_dir = temp_dir("rginx-ocsp-unknown-status");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let leaf = generate_leaf_cert("localhost", &ca);
        let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
        let response = build_ocsp_response_for_certificate_with_signer(
            &cert_path,
            TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
            Some(TimeOffset::After(Duration::from_secs(24 * 60 * 60))),
            TimeOffset::Before(Duration::from_secs(60)),
            RasnCertStatus::Unknown(()),
            OcspResponseSigner::Issuer(&ca),
            None,
            false,
            false,
        );

        let error = validate_ocsp_response_for_certificate(&cert_path, &response)
            .expect_err("unknown OCSP cert status should be rejected");
        assert!(error.to_string().contains("unknown certificate status"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn validate_ocsp_response_rejects_invalid_signature() {
        let temp_dir = temp_dir("rginx-ocsp-invalid-signature");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let leaf = generate_leaf_cert("localhost", &ca);
        let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
        let response = build_ocsp_response_for_certificate_with_signer(
            &cert_path,
            TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
            Some(TimeOffset::After(Duration::from_secs(24 * 60 * 60))),
            TimeOffset::Before(Duration::from_secs(60)),
            RasnCertStatus::Good,
            OcspResponseSigner::Issuer(&ca),
            None,
            false,
            true,
        );

        let error = validate_ocsp_response_for_certificate(&cert_path, &response)
            .expect_err("invalid OCSP signature should be rejected");
        assert!(error.to_string().contains("invalid responder signature"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn validate_ocsp_response_accepts_authorized_delegated_signer() {
        let temp_dir = temp_dir("rginx-ocsp-delegated-signer");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let leaf = generate_leaf_cert("localhost", &ca);
        let responder = generate_ocsp_responder_cert("ocsp-responder", &ca, true, true);
        let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
        let response = build_ocsp_response_for_certificate_with_signer(
            &cert_path,
            TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
            Some(TimeOffset::After(Duration::from_secs(24 * 60 * 60))),
            TimeOffset::Before(Duration::from_secs(60)),
            RasnCertStatus::Good,
            OcspResponseSigner::Delegated(&responder),
            None,
            false,
            false,
        );

        validate_ocsp_response_for_certificate(&cert_path, &response)
            .expect("authorized delegated responder should be accepted");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn validate_ocsp_response_rejects_delegated_signer_without_ocsp_eku() {
        let temp_dir = temp_dir("rginx-ocsp-delegated-no-eku");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let leaf = generate_leaf_cert("localhost", &ca);
        let responder = generate_ocsp_responder_cert("ocsp-responder", &ca, false, true);
        let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
        let response = build_ocsp_response_for_certificate_with_signer(
            &cert_path,
            TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
            Some(TimeOffset::After(Duration::from_secs(24 * 60 * 60))),
            TimeOffset::Before(Duration::from_secs(60)),
            RasnCertStatus::Good,
            OcspResponseSigner::Delegated(&responder),
            None,
            false,
            false,
        );

        let error = validate_ocsp_response_for_certificate(&cert_path, &response)
            .expect_err("delegated responder without EKU should be rejected");
        assert!(error.to_string().contains("OCSP signing extended key usage"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn validate_ocsp_response_rejects_multiple_matching_single_responses() {
        let temp_dir = temp_dir("rginx-ocsp-duplicate-matches");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let leaf = generate_leaf_cert("localhost", &ca);
        let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
        let response = build_ocsp_response_for_certificate_with_signer(
            &cert_path,
            TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
            Some(TimeOffset::After(Duration::from_secs(24 * 60 * 60))),
            TimeOffset::Before(Duration::from_secs(60)),
            RasnCertStatus::Good,
            OcspResponseSigner::Issuer(&ca),
            None,
            true,
            false,
        );

        let error = validate_ocsp_response_for_certificate(&cert_path, &response)
            .expect_err("multiple matching SingleResponses should be rejected");
        assert!(error.to_string().contains("multiple matching SingleResponses"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn build_ocsp_request_includes_nonce_when_enabled() {
        let temp_dir = temp_dir("rginx-ocsp-request-nonce");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let leaf = generate_leaf_cert("localhost", &ca);
        let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
        let (request, nonce) =
            build_ocsp_request_for_certificate_with_options(&cert_path, OcspNonceMode::Required)
                .expect("OCSP request should build with nonce");
        let request: RasnOcspRequest =
            rasn::der::decode(&request).expect("OCSP request should decode");

        let request_nonce =
            extract_ocsp_nonce(&cert_path, request.tbs_request.request_extensions.as_ref())
                .expect("request nonce should parse")
                .expect("request nonce should exist");
        assert_eq!(request_nonce, nonce.expect("nonce should be generated"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn validate_ocsp_response_rejects_missing_required_nonce() {
        let temp_dir = temp_dir("rginx-ocsp-missing-required-nonce");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let leaf = generate_leaf_cert("localhost", &ca);
        let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
        let response = build_ocsp_response_for_certificate_with_signer(
            &cert_path,
            TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
            Some(TimeOffset::After(Duration::from_secs(24 * 60 * 60))),
            TimeOffset::Before(Duration::from_secs(60)),
            RasnCertStatus::Good,
            OcspResponseSigner::Issuer(&ca),
            None,
            false,
            false,
        );

        let error = validate_ocsp_response_for_certificate_with_options(
            &cert_path,
            &response,
            Some(b"expected-nonce"),
            OcspNonceMode::Required,
            OcspResponderPolicy::IssuerOrDelegated,
        )
        .expect_err("missing required nonce should be rejected");
        assert!(error.to_string().contains("did not echo the required nonce"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn validate_ocsp_response_rejects_mismatched_required_nonce() {
        let temp_dir = temp_dir("rginx-ocsp-mismatched-required-nonce");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let leaf = generate_leaf_cert("localhost", &ca);
        let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
        let response = build_ocsp_response_for_certificate_with_signer(
            &cert_path,
            TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
            Some(TimeOffset::After(Duration::from_secs(24 * 60 * 60))),
            TimeOffset::Before(Duration::from_secs(60)),
            RasnCertStatus::Good,
            OcspResponseSigner::Issuer(&ca),
            Some(b"response-nonce"),
            false,
            false,
        );

        let error = validate_ocsp_response_for_certificate_with_options(
            &cert_path,
            &response,
            Some(b"expected-nonce"),
            OcspNonceMode::Required,
            OcspResponderPolicy::IssuerOrDelegated,
        )
        .expect_err("mismatched nonce should be rejected");
        assert!(error.to_string().contains("mismatched nonce"));

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn validate_ocsp_response_accepts_missing_preferred_nonce() {
        let temp_dir = temp_dir("rginx-ocsp-preferred-nonce-missing");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let leaf = generate_leaf_cert("localhost", &ca);
        let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);
        let response = build_ocsp_response_for_certificate_with_signer(
            &cert_path,
            TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
            Some(TimeOffset::After(Duration::from_secs(24 * 60 * 60))),
            TimeOffset::Before(Duration::from_secs(60)),
            RasnCertStatus::Good,
            OcspResponseSigner::Issuer(&ca),
            None,
            false,
            false,
        );

        validate_ocsp_response_for_certificate_with_options(
            &cert_path,
            &response,
            Some(b"expected-nonce"),
            OcspNonceMode::Preferred,
            OcspResponderPolicy::IssuerOrDelegated,
        )
        .expect("preferred nonce should allow missing echoed nonce");

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn ocsp_responder_urls_for_certificate_extracts_aia_ocsp_uri() {
        let temp_dir = temp_dir("rginx-ocsp-aia-responder-url");
        std::fs::create_dir_all(&temp_dir).expect("temp dir should exist");

        let ca = generate_ca_cert("ocsp-test-ca");
        let leaf =
            generate_leaf_cert_with_ocsp_aia("localhost", &ca, "http://127.0.0.1:19090/ocsp");
        let cert_path = write_cert_chain(&temp_dir, "server", &leaf, &ca);

        let urls = ocsp_responder_urls_for_certificate(&cert_path)
            .expect("AIA OCSP responder discovery should succeed");
        assert_eq!(urls, vec!["http://127.0.0.1:19090/ocsp".to_string()]);

        let _ = std::fs::remove_dir_all(temp_dir);
    }

    fn generate_ca_cert(common_name: &str) -> TestCertifiedKey {
        let mut params =
            CertificateParams::new(vec![common_name.to_string()]).expect("CA params should build");
        params.distinguished_name.push(DnType::CommonName, common_name);
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        let signing_key = KeyPair::generate().expect("CA keypair should generate");
        let cert = params.self_signed(&signing_key).expect("CA certificate should self-sign");
        TestCertifiedKey { cert, signing_key, params }
    }

    fn generate_leaf_cert(common_name: &str, issuer: &TestCertifiedKey) -> TestCertifiedKey {
        let mut params = CertificateParams::new(vec![common_name.to_string()])
            .expect("leaf params should build");
        params.distinguished_name.push(DnType::CommonName, common_name);
        let signing_key = KeyPair::generate().expect("leaf keypair should generate");
        let cert = params
            .signed_by(&signing_key, &issuer.issuer())
            .expect("leaf certificate should be signed");
        TestCertifiedKey { cert, signing_key, params }
    }

    fn generate_leaf_cert_with_ocsp_aia(
        common_name: &str,
        issuer: &TestCertifiedKey,
        responder_url: &str,
    ) -> TestCertifiedKey {
        let mut params = CertificateParams::new(vec![common_name.to_string()])
            .expect("leaf params should build");
        params.distinguished_name.push(DnType::CommonName, common_name);
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.custom_extensions.push(CustomExtension::from_oid_content(
            &[1, 3, 6, 1, 5, 5, 7, 1, 1],
            authority_info_access_extension_value(responder_url),
        ));
        let signing_key = KeyPair::generate().expect("leaf keypair should generate");
        let cert = params
            .signed_by(&signing_key, &issuer.issuer())
            .expect("leaf certificate should be signed");
        TestCertifiedKey { cert, signing_key, params }
    }

    fn generate_ocsp_responder_cert(
        common_name: &str,
        issuer: &TestCertifiedKey,
        ocsp_signing: bool,
        digital_signature: bool,
    ) -> TestCertifiedKey {
        let mut params = CertificateParams::new(vec![common_name.to_string()])
            .expect("OCSP responder params should build");
        params.distinguished_name.push(DnType::CommonName, common_name);
        if ocsp_signing {
            params.extended_key_usages = vec![ExtendedKeyUsagePurpose::OcspSigning];
        }
        if digital_signature {
            params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        }
        let signing_key = KeyPair::generate().expect("OCSP responder keypair should generate");
        let cert = params
            .signed_by(&signing_key, &issuer.issuer())
            .expect("OCSP responder certificate should be signed");
        TestCertifiedKey { cert, signing_key, params }
    }

    fn write_cert_chain(
        temp_dir: &Path,
        name: &str,
        leaf: &TestCertifiedKey,
        ca: &TestCertifiedKey,
    ) -> PathBuf {
        let path = temp_dir.join(format!("{name}.crt"));
        std::fs::write(&path, format!("{}{}", leaf.cert.pem(), ca.cert.pem()))
            .expect("certificate chain should be written");
        path
    }

    fn write_private_key(temp_dir: &Path, name: &str, leaf: &TestCertifiedKey) -> PathBuf {
        let path = temp_dir.join(format!("{name}.key"));
        std::fs::write(&path, leaf.signing_key.serialize_pem())
            .expect("private key should be written");
        path
    }

    fn build_ocsp_response_for_certificate(cert_path: &Path, issuer: &TestCertifiedKey) -> Vec<u8> {
        build_ocsp_response_for_certificate_with_signer(
            cert_path,
            TimeOffset::Before(Duration::from_secs(24 * 60 * 60)),
            Some(TimeOffset::After(Duration::from_secs(24 * 60 * 60))),
            TimeOffset::Before(Duration::from_secs(60)),
            RasnCertStatus::Good,
            OcspResponseSigner::Issuer(issuer),
            None,
            false,
            false,
        )
    }

    fn build_ocsp_response_for_certificate_with_offsets(
        cert_path: &Path,
        issuer: &TestCertifiedKey,
        this_update_offset: TimeOffset,
        next_update_offset: TimeOffset,
    ) -> Vec<u8> {
        build_ocsp_response_for_certificate_with_signer(
            cert_path,
            this_update_offset,
            Some(next_update_offset),
            this_update_offset,
            RasnCertStatus::Good,
            OcspResponseSigner::Issuer(issuer),
            None,
            false,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_ocsp_response_for_certificate_with_signer(
        cert_path: &Path,
        this_update_offset: TimeOffset,
        next_update_offset: Option<TimeOffset>,
        produced_at_offset: TimeOffset,
        cert_status: RasnCertStatus,
        signer: OcspResponseSigner<'_>,
        response_nonce: Option<&[u8]>,
        duplicate_matching_response: bool,
        tamper_signature: bool,
    ) -> Vec<u8> {
        let certs =
            load_certificate_chain_from_path(cert_path).expect("certificate chain should load");
        let cert_id =
            build_rasn_ocsp_cert_id_from_chain(&certs, cert_path).expect("CertId should build");
        let now = SystemTime::now();
        let this_update = ocsp_time_with_offset(now, this_update_offset);
        let produced_at = ocsp_time_with_offset(now, produced_at_offset);
        let next_update = next_update_offset.map(|offset| ocsp_time_with_offset(now, offset));
        let mut responses = vec![RasnSingleResponse {
            cert_id: cert_id.clone(),
            cert_status: cert_status.clone(),
            this_update,
            next_update,
            single_extensions: None,
        }];
        if duplicate_matching_response {
            responses.push(RasnSingleResponse {
                cert_id,
                cert_status,
                this_update,
                next_update,
                single_extensions: None,
            });
        }

        let tbs_response_data = RasnResponseData {
            version: Integer::from(0),
            responder_id: signer.responder_id(),
            produced_at,
            responses,
            response_extensions: response_nonce
                .map(build_ocsp_nonce_extension)
                .transpose()
                .expect("response nonce should encode")
                .map(|extension| vec![extension].into()),
        };
        let tbs_der =
            rasn::der::encode(&tbs_response_data).expect("response data should encode for signing");
        let mut signature = signer.signing_key().sign(&tbs_der).expect("OCSP response should sign");
        if tamper_signature {
            signature[0] ^= 0x55;
        }

        let basic = RasnBasicOcspResponse {
            tbs_response_data,
            signature_algorithm: test_signature_algorithm(signer.signing_key()),
            signature: BitString::from_slice(&signature),
            certs: signer.embedded_certs(),
        };
        let basic_der = rasn::der::encode(&basic).expect("basic OCSP response should encode");
        rasn::der::encode(&RasnOcspResponse {
            status: RasnOcspResponseStatus::Successful,
            bytes: Some(RasnResponseBytes {
                r#type: basic_ocsp_response_type_oid(),
                response: OctetString::from_slice(&basic_der),
            }),
        })
        .expect("OCSP response should encode")
    }

    #[derive(Clone, Copy)]
    enum TimeOffset {
        Before(Duration),
        After(Duration),
    }

    enum OcspResponseSigner<'a> {
        Issuer(&'a TestCertifiedKey),
        Delegated(&'a TestCertifiedKey),
    }

    impl<'a> OcspResponseSigner<'a> {
        fn signing_key(&self) -> &KeyPair {
            match self {
                Self::Issuer(key) | Self::Delegated(key) => &key.signing_key,
            }
        }

        fn responder_id(&self) -> RasnResponderId {
            match self {
                Self::Issuer(key) | Self::Delegated(key) => {
                    responder_id_for_certificate(key.cert.der().as_ref())
                }
            }
        }

        fn embedded_certs(&self) -> Option<Vec<rasn_pkix::Certificate>> {
            match self {
                Self::Delegated(key) => Some(vec![
                    rasn::der::decode(key.cert.der().as_ref())
                        .expect("delegated responder certificate should decode"),
                ]),
                _ => None,
            }
        }
    }

    fn ocsp_time_with_offset(base: SystemTime, offset: TimeOffset) -> GeneralizedTime {
        let time = match offset {
            TimeOffset::Before(duration) => {
                base.checked_sub(duration).expect("time offset should stay after unix epoch")
            }
            TimeOffset::After(duration) => base + duration,
        };
        generalized_time_from_system_time(time)
    }

    fn responder_id_for_certificate(cert_der: &[u8]) -> RasnResponderId {
        let cert: RasnCertificate = rasn::der::decode(cert_der).expect("certificate should decode");
        RasnResponderId::ByKey(OctetString::from(
            Sha1::digest(subject_public_key_bytes(&cert)).to_vec(),
        ))
    }

    fn authority_info_access_extension_value(responder_url: &str) -> Vec<u8> {
        der_sequence([der_sequence([
            vec![0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01],
            der_context_6_ia5_string(responder_url.as_bytes()),
        ])])
    }

    fn der_sequence<const N: usize>(elements: [Vec<u8>; N]) -> Vec<u8> {
        let payload = elements.into_iter().flatten().collect::<Vec<_>>();
        der_wrap(0x30, payload)
    }

    fn der_context_6_ia5_string(bytes: &[u8]) -> Vec<u8> {
        der_wrap(0x86, bytes.to_vec())
    }

    fn der_wrap(tag: u8, payload: Vec<u8>) -> Vec<u8> {
        let mut encoded = Vec::new();
        encoded.push(tag);
        encoded.extend(der_length(payload.len()));
        encoded.extend(payload);
        encoded
    }

    fn der_length(length: usize) -> Vec<u8> {
        if length < 0x80 {
            return vec![length as u8];
        }

        let bytes =
            length.to_be_bytes().into_iter().skip_while(|byte| *byte == 0).collect::<Vec<_>>();
        let mut encoded = Vec::with_capacity(bytes.len() + 1);
        encoded.push(0x80 | (bytes.len() as u8));
        encoded.extend(bytes);
        encoded
    }

    fn test_signature_algorithm(key: &KeyPair) -> rasn_pkix::AlgorithmIdentifier {
        let der = if key.algorithm() == &PKCS_ECDSA_P256_SHA256 {
            &[0x30, 0x0a, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02][..]
        } else if key.algorithm() == &PKCS_RSA_SHA256 {
            &[
                0x30, 0x0d, 0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b, 0x05,
                0x00,
            ][..]
        } else if key.algorithm() == &PKCS_ED25519 {
            &[0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70][..]
        } else {
            panic!("unsupported OCSP test signature algorithm");
        };
        rasn::der::decode(der).expect("signature algorithm should decode")
    }

    fn temp_dir(prefix: &str) -> PathBuf {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!("{prefix}-{unique}-{id}"))
    }
}
