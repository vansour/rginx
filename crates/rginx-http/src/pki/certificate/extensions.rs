use rasn_pkix::{
    AuthorityKeyIdentifier, BasicConstraints, ExtKeyUsageSyntax, Extension, GeneralName, KeyUsage,
    SubjectAltName, SubjectKeyIdentifier,
};

use super::helpers::hex_string;
use super::{
    OID_EKU_ANY, OID_EKU_CLIENT_AUTH, OID_EKU_CODE_SIGNING, OID_EKU_EMAIL_PROTECTION,
    OID_EKU_OCSP_SIGNING, OID_EKU_SERVER_AUTH, OID_EKU_TIME_STAMPING,
    OID_EXT_AUTHORITY_KEY_IDENTIFIER, OID_EXT_BASIC_CONSTRAINTS, OID_EXT_EXTENDED_KEY_USAGE,
    OID_EXT_KEY_USAGE, OID_EXT_SUBJECT_ALT_NAME, OID_EXT_SUBJECT_KEY_IDENTIFIER,
};

pub(super) fn subject_key_identifier(extensions: Option<&[Extension]>) -> Option<String> {
    let extension = find_extension(extensions, OID_EXT_SUBJECT_KEY_IDENTIFIER)?;
    let value: SubjectKeyIdentifier = rasn::der::decode(extension.extn_value.as_ref()).ok()?;
    Some(hex_string(value.as_ref()))
}

pub(super) fn authority_key_identifier(extensions: Option<&[Extension]>) -> Option<String> {
    let extension = find_extension(extensions, OID_EXT_AUTHORITY_KEY_IDENTIFIER)?;
    let value: AuthorityKeyIdentifier = rasn::der::decode(extension.extn_value.as_ref()).ok()?;
    value.key_identifier.as_ref().map(|identifier| hex_string(identifier.as_ref()))
}

pub(super) fn subject_alt_dns_names(extensions: Option<&[Extension]>) -> Vec<String> {
    let Some(extension) = find_extension(extensions, OID_EXT_SUBJECT_ALT_NAME) else {
        return Vec::new();
    };
    let Ok(value) = rasn::der::decode::<SubjectAltName>(extension.extn_value.as_ref()) else {
        return Vec::new();
    };
    value
        .into_iter()
        .filter_map(|name| match name {
            GeneralName::DnsName(dns) => Some(dns.to_string()),
            _ => None,
        })
        .collect()
}

pub(super) fn basic_constraints(extensions: Option<&[Extension]>) -> Option<BasicConstraints> {
    let extension = find_extension(extensions, OID_EXT_BASIC_CONSTRAINTS)?;
    rasn::der::decode(extension.extn_value.as_ref()).ok()
}

#[derive(Debug, Clone)]
pub(super) struct ParsedKeyUsage {
    pub(super) digital_signature: bool,
    pub(super) key_encipherment: bool,
    pub(super) key_agreement: bool,
    pub(super) key_cert_sign: bool,
    names: Vec<&'static str>,
}

pub(super) fn key_usage(extensions: Option<&[Extension]>) -> Option<ParsedKeyUsage> {
    let extension = find_extension(extensions, OID_EXT_KEY_USAGE)?;
    let bits: KeyUsage = rasn::der::decode(extension.extn_value.as_ref()).ok()?;
    let bit = |index: usize| bits.get(index).map(|value| *value).unwrap_or(false);
    let mut names = Vec::new();
    if bit(0) {
        names.push("digitalSignature");
    }
    if bit(1) {
        names.push("nonRepudiation");
    }
    if bit(2) {
        names.push("keyEncipherment");
    }
    if bit(3) {
        names.push("dataEncipherment");
    }
    if bit(4) {
        names.push("keyAgreement");
    }
    if bit(5) {
        names.push("keyCertSign");
    }
    if bit(6) {
        names.push("cRLSign");
    }
    if bit(7) {
        names.push("encipherOnly");
    }
    if bit(8) {
        names.push("decipherOnly");
    }

    Some(ParsedKeyUsage {
        digital_signature: bit(0),
        key_encipherment: bit(2),
        key_agreement: bit(4),
        key_cert_sign: bit(5),
        names,
    })
}

pub(super) fn describe_key_usage(value: &ParsedKeyUsage) -> String {
    value.names.join(",")
}

pub(super) fn extended_key_usage(extensions: Option<&[Extension]>) -> Option<Vec<String>> {
    let extension = find_extension(extensions, OID_EXT_EXTENDED_KEY_USAGE)?;
    let values: ExtKeyUsageSyntax = rasn::der::decode(extension.extn_value.as_ref()).ok()?;
    Some(
        values
            .into_iter()
            .map(|oid| match oid.to_string().as_str() {
                OID_EKU_ANY => "any".to_string(),
                OID_EKU_SERVER_AUTH => "server_auth".to_string(),
                OID_EKU_CLIENT_AUTH => "client_auth".to_string(),
                OID_EKU_CODE_SIGNING => "code_signing".to_string(),
                OID_EKU_EMAIL_PROTECTION => "email_protection".to_string(),
                OID_EKU_TIME_STAMPING => "time_stamping".to_string(),
                OID_EKU_OCSP_SIGNING => "ocsp_signing".to_string(),
                other => other.to_string(),
            })
            .collect(),
    )
}

fn find_extension<'a>(extensions: Option<&'a [Extension]>, oid: &str) -> Option<&'a Extension> {
    extensions?.iter().find(|extension| extension.extn_id.to_string() == oid)
}
