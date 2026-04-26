use super::*;

pub(super) fn basic_ocsp_response_type_oid() -> ObjectIdentifier {
    ObjectIdentifier::new(vec![1, 3, 6, 1, 5, 5, 7, 48, 1, 1])
        .expect("basic OCSP response type OID should be valid")
}

pub(super) fn parse_leaf_and_issuer_certificates(
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

pub(super) fn build_rasn_ocsp_cert_id_from_chain(
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

pub(super) fn subject_public_key_bytes(cert: &RasnCertificate) -> &[u8] {
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

pub(super) fn certificate_extended_key_usage(
    cert: &RasnCertificate,
) -> Result<Option<ExtKeyUsageSyntax>> {
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

pub(super) fn certificate_key_usage(cert: &RasnCertificate) -> Result<Option<RasnKeyUsage>> {
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

pub(super) fn bit_string_flag(value: &RasnKeyUsage, index: usize) -> bool {
    value.get(index).map(|bit| *bit).unwrap_or(false)
}

pub(super) fn hex_string(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
}

pub(super) fn algorithm_identifier_value_bytes(
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

pub(super) fn signature_bytes<'a>(
    path: &Path,
    signature: &'a rasn::types::BitString,
) -> Result<&'a [u8]> {
    if !signature.len().is_multiple_of(8) {
        return Err(Error::Server(format!(
            "failed to parse OCSP signature value for certificate `{}`: signature BIT STRING contains unused bits",
            path.display()
        )));
    }
    Ok(signature.as_raw_slice())
}
