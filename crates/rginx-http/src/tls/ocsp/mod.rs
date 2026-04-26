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
    if !signature.len().is_multiple_of(8) {
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

#[cfg(test)]
mod tests;
