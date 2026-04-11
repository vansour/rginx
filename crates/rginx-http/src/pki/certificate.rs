use std::collections::HashSet;
use std::io::BufReader;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rasn::types::{Integer, IntegerType};
use rasn_pkix::{
    AuthorityKeyIdentifier, BasicConstraints, Certificate, DirectoryString, ExtKeyUsageSyntax,
    Extension, GeneralName, KeyUsage, Name, SubjectAltName, SubjectKeyIdentifier, Time,
};
use sha2::{Digest, Sha256};

use crate::client_ip::TlsClientIdentity;

const TLS_EXPIRY_WARNING_DAYS: i64 = 30;

const OID_ATTR_COMMON_NAME: &str = "2.5.4.3";
const OID_ATTR_COUNTRY_NAME: &str = "2.5.4.6";
const OID_ATTR_LOCALITY_NAME: &str = "2.5.4.7";
const OID_ATTR_STATE_OR_PROVINCE_NAME: &str = "2.5.4.8";
const OID_ATTR_ORGANIZATION_NAME: &str = "2.5.4.10";
const OID_ATTR_ORGANIZATIONAL_UNIT_NAME: &str = "2.5.4.11";
const OID_ATTR_DOMAIN_COMPONENT: &str = "0.9.2342.19200300.100.1.25";
const OID_ATTR_EMAIL_ADDRESS: &str = "1.2.840.113549.1.9.1";

const OID_EXT_SUBJECT_KEY_IDENTIFIER: &str = "2.5.29.14";
const OID_EXT_KEY_USAGE: &str = "2.5.29.15";
const OID_EXT_SUBJECT_ALT_NAME: &str = "2.5.29.17";
const OID_EXT_BASIC_CONSTRAINTS: &str = "2.5.29.19";
const OID_EXT_AUTHORITY_KEY_IDENTIFIER: &str = "2.5.29.35";
const OID_EXT_EXTENDED_KEY_USAGE: &str = "2.5.29.37";

const OID_EKU_ANY: &str = "2.5.29.37.0";
const OID_EKU_SERVER_AUTH: &str = "1.3.6.1.5.5.7.3.1";
const OID_EKU_CLIENT_AUTH: &str = "1.3.6.1.5.5.7.3.2";
const OID_EKU_CODE_SIGNING: &str = "1.3.6.1.5.5.7.3.3";
const OID_EKU_EMAIL_PROTECTION: &str = "1.3.6.1.5.5.7.3.4";
const OID_EKU_TIME_STAMPING: &str = "1.3.6.1.5.5.7.3.8";
const OID_EKU_OCSP_SIGNING: &str = "1.3.6.1.5.5.7.3.9";

#[derive(Debug, Clone)]
pub(crate) struct InspectedCertificate {
    pub(crate) subject: Option<String>,
    pub(crate) issuer: Option<String>,
    pub(crate) serial_number: Option<String>,
    pub(crate) san_dns_names: Vec<String>,
    pub(crate) fingerprint_sha256: Option<String>,
    pub(crate) subject_key_identifier: Option<String>,
    pub(crate) authority_key_identifier: Option<String>,
    pub(crate) is_ca: Option<bool>,
    pub(crate) path_len_constraint: Option<u32>,
    pub(crate) key_usage: Option<String>,
    pub(crate) extended_key_usage: Vec<String>,
    pub(crate) not_before_unix_ms: Option<u64>,
    pub(crate) not_after_unix_ms: Option<u64>,
    pub(crate) expires_in_days: Option<i64>,
    pub(crate) chain_length: usize,
    pub(crate) chain_subjects: Vec<String>,
    pub(crate) chain_diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedClientIdentity {
    pub(crate) subject: Option<String>,
    pub(crate) issuer: Option<String>,
    pub(crate) serial_number: Option<String>,
    pub(crate) san_dns_names: Vec<String>,
    pub(crate) chain_length: usize,
    pub(crate) chain_subjects: Vec<String>,
}

impl From<ParsedClientIdentity> for TlsClientIdentity {
    fn from(identity: ParsedClientIdentity) -> Self {
        Self {
            subject: identity.subject,
            issuer: identity.issuer,
            serial_number: identity.serial_number,
            san_dns_names: identity.san_dns_names,
            chain_length: identity.chain_length,
            chain_subjects: identity.chain_subjects,
        }
    }
}

pub(crate) fn parse_tls_client_identity<'a>(
    der_chain: impl IntoIterator<Item = &'a [u8]>,
) -> ParsedClientIdentity {
    let mut identity = ParsedClientIdentity {
        subject: None,
        issuer: None,
        serial_number: None,
        san_dns_names: Vec::new(),
        chain_length: 0,
        chain_subjects: Vec::new(),
    };

    for (index, der) in der_chain.into_iter().enumerate() {
        identity.chain_length += 1;
        if let Some(cert) = decode_certificate(der) {
            let extensions = cert.tbs_certificate.extensions.as_ref().map(|value| value.as_slice());
            let subject = name_to_string(&cert.tbs_certificate.subject);
            identity.chain_subjects.push(subject.clone());
            if index == 0 {
                identity.subject = Some(subject);
                identity.issuer = Some(name_to_string(&cert.tbs_certificate.issuer));
                identity.serial_number =
                    Some(integer_to_serial_string(&cert.tbs_certificate.serial_number));
                identity.san_dns_names = subject_alt_dns_names(extensions);
            }
        }
    }

    identity
}

pub(crate) fn inspect_certificate(path: &Path) -> Option<InspectedCertificate> {
    let certs = load_certificate_chain_der(path).ok()?;
    if certs.is_empty() {
        return None;
    }

    let now_secs = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs() as i64;
    let mut chain_subjects = Vec::new();
    let mut chain_entries = Vec::new();
    let mut chain_diagnostics = Vec::new();
    let mut seen_fingerprints = HashSet::new();

    for (index, der) in certs.iter().enumerate() {
        let fingerprint_sha256 = fingerprint_sha256(der.as_ref());
        if !seen_fingerprints.insert(fingerprint_sha256.clone()) {
            chain_diagnostics.push(format!(
                "duplicate_certificate_in_chain cert[{index}] sha256={fingerprint_sha256}"
            ));
        }

        let Some(cert) = decode_certificate(der.as_ref()) else {
            chain_diagnostics.push(format!("cert[{index}] could_not_be_parsed_as_x509"));
            continue;
        };

        let extensions = cert.tbs_certificate.extensions.as_ref().map(|value| value.as_slice());
        let subject = name_to_string(&cert.tbs_certificate.subject);
        let issuer = name_to_string(&cert.tbs_certificate.issuer);
        let expires_in_days = time_to_unix_secs(cert.tbs_certificate.validity.not_after)
            .map(|not_after| (not_after - now_secs).div_euclid(86_400));
        let basic_constraints = basic_constraints(extensions);
        let key_usage = key_usage(extensions);
        let extended_key_usage = extended_key_usage(extensions);
        let subject_key_identifier = subject_key_identifier(extensions);
        let authority_key_identifier = authority_key_identifier(extensions);

        if let Some(expires_in_days) = expires_in_days {
            if expires_in_days < 0 {
                chain_diagnostics.push(format!("cert[{index}] expired"));
            } else if expires_in_days <= TLS_EXPIRY_WARNING_DAYS {
                chain_diagnostics.push(format!("cert[{index}] expires_in_{expires_in_days}d"));
            }
        }
        if index == 0 && basic_constraints.as_ref().is_some_and(|constraints| constraints.ca) {
            chain_diagnostics.push("leaf_certificate_is_marked_as_ca".to_string());
        }
        if index == 0
            && key_usage.as_ref().is_some_and(|usage| {
                !usage.digital_signature && !usage.key_encipherment && !usage.key_agreement
            })
        {
            chain_diagnostics.push("leaf_key_usage_may_not_allow_tls_server_auth".to_string());
        }
        if index == 0
            && extended_key_usage.as_ref().is_some_and(|usage| {
                !usage.iter().any(|value| value == "any" || value == "server_auth")
            })
        {
            chain_diagnostics.push("leaf_missing_server_auth_eku".to_string());
        }
        if index > 0 && !basic_constraints.as_ref().is_some_and(|constraints| constraints.ca) {
            chain_diagnostics.push(format!("cert[{index}] intermediate_or_root_not_marked_as_ca"));
        }
        if index > 0 && key_usage.as_ref().is_some_and(|usage| !usage.key_cert_sign) {
            chain_diagnostics
                .push(format!("cert[{index}] intermediate_or_root_missing_key_cert_sign"));
        }

        chain_subjects.push(subject.clone());
        chain_entries.push(InspectedCertificate {
            subject: Some(subject),
            issuer: Some(issuer),
            serial_number: Some(integer_to_serial_string(&cert.tbs_certificate.serial_number)),
            san_dns_names: subject_alt_dns_names(extensions),
            fingerprint_sha256: Some(fingerprint_sha256),
            subject_key_identifier,
            authority_key_identifier,
            is_ca: basic_constraints.as_ref().map(|constraints| constraints.ca),
            path_len_constraint: basic_constraints
                .as_ref()
                .and_then(|constraints| constraints.path_len_constraint.clone())
                .as_ref()
                .and_then(integer_to_u32),
            key_usage: key_usage.as_ref().map(describe_key_usage),
            extended_key_usage: extended_key_usage.unwrap_or_default(),
            not_before_unix_ms: time_to_unix_ms(cert.tbs_certificate.validity.not_before),
            not_after_unix_ms: time_to_unix_ms(cert.tbs_certificate.validity.not_after),
            expires_in_days,
            chain_length: certs.len(),
            chain_subjects: Vec::new(),
            chain_diagnostics: Vec::new(),
        });
    }

    if chain_entries.len() == certs.len() {
        for index in 0..chain_entries.len().saturating_sub(1) {
            let issuer = chain_entries[index].issuer.as_deref();
            let next_subject = chain_entries[index + 1].subject.as_deref();
            if issuer != next_subject {
                chain_diagnostics.push(format!(
                    "chain_link_mismatch cert[{index}]_issuer_to_cert[{}]_subject",
                    index + 1
                ));
            }
            if let (Some(aki), Some(ski)) = (
                chain_entries[index].authority_key_identifier.as_deref(),
                chain_entries[index + 1].subject_key_identifier.as_deref(),
            ) && aki != ski
            {
                chain_diagnostics
                    .push(format!("chain_aki_ski_mismatch cert[{index}]_to_cert[{}]", index + 1));
            }
            if let Some(path_len_constraint) = chain_entries[index + 1].path_len_constraint {
                let descendant_ca_certs = chain_entries[..index + 1]
                    .iter()
                    .filter(|entry| entry.is_ca == Some(true))
                    .count() as u32;
                if descendant_ca_certs > path_len_constraint {
                    chain_diagnostics.push(format!(
                        "cert[{}] path_len_constraint_exceeded descendant_ca_certs={} path_len_constraint={}",
                        index + 1,
                        descendant_ca_certs,
                        path_len_constraint
                    ));
                }
            }
        }
    } else if certs.len() > 1 {
        chain_diagnostics
            .push("chain_link_checks_skipped_due_to_unparseable_certificate".to_string());
    }

    if let Some(leaf) = chain_entries.first() {
        if certs.len() == 1 {
            if leaf.subject != leaf.issuer {
                chain_diagnostics
                    .push("chain_incomplete_single_non_self_signed_certificate".to_string());
            }
        } else if let Some(last) = chain_entries.last()
            && last.subject != last.issuer
        {
            chain_diagnostics.push("chain_incomplete_non_self_signed_top_certificate".to_string());
        }
    }

    let leaf = chain_entries.into_iter().next()?;
    Some(InspectedCertificate {
        chain_length: certs.len(),
        chain_subjects,
        chain_diagnostics,
        ..leaf
    })
}

fn decode_certificate(bytes: &[u8]) -> Option<Certificate> {
    rasn::der::decode(bytes).ok()
}

fn load_certificate_chain_der(path: &Path) -> std::io::Result<Vec<Vec<u8>>> {
    let file = std::fs::File::open(path)?;
    let mut reader = BufReader::new(file);
    let certs = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    if !certs.is_empty() {
        return Ok(certs.into_iter().map(|cert| cert.as_ref().to_vec()).collect());
    }
    Ok(vec![std::fs::read(path)?])
}

fn name_to_string(name: &Name) -> String {
    let mut components = Vec::new();
    let Name::RdnSequence(rdns) = name;
    for rdn in rdns {
        let mut rdn_components = rdn
            .to_vec()
            .into_iter()
            .map(|attribute| {
                let oid = attribute.r#type.to_string();
                let label = attribute_label(&oid);
                let value = decode_attribute_value(&oid, &attribute.value);
                format!("{label}={value}")
            })
            .collect::<Vec<_>>();
        rdn_components.sort();
        components.extend(rdn_components);
    }
    if components.is_empty() { "-".to_string() } else { components.join(",") }
}

fn attribute_label(oid: &str) -> String {
    match oid {
        OID_ATTR_COMMON_NAME => "CN".to_string(),
        OID_ATTR_COUNTRY_NAME => "C".to_string(),
        OID_ATTR_LOCALITY_NAME => "L".to_string(),
        OID_ATTR_STATE_OR_PROVINCE_NAME => "ST".to_string(),
        OID_ATTR_ORGANIZATION_NAME => "O".to_string(),
        OID_ATTR_ORGANIZATIONAL_UNIT_NAME => "OU".to_string(),
        OID_ATTR_DOMAIN_COMPONENT => "DC".to_string(),
        OID_ATTR_EMAIL_ADDRESS => "emailAddress".to_string(),
        _ => oid.to_string(),
    }
}

fn decode_attribute_value(oid: &str, value: &rasn::types::Any) -> String {
    match oid {
        OID_ATTR_COUNTRY_NAME => decode_printable_string(value.as_bytes()),
        OID_ATTR_DOMAIN_COMPONENT | OID_ATTR_EMAIL_ADDRESS => decode_ia5_string(value.as_bytes()),
        _ => decode_directory_or_string(value.as_bytes()),
    }
}

fn decode_directory_or_string(bytes: &[u8]) -> String {
    if let Ok(value) = rasn::der::decode::<DirectoryString>(bytes) {
        return match value {
            DirectoryString::Printable(value) => bytes_to_lossy_string(value.as_bytes()),
            DirectoryString::Utf8(value) => value,
            DirectoryString::Teletex(value) => codepoints_to_string(value.iter().copied()),
            DirectoryString::Bmp(value) => {
                codepoints_to_string(value.iter().map(|&ch| u32::from(ch)))
            }
            DirectoryString::Universal(value) => value.to_string(),
        };
    }

    if let Ok(value) = rasn::der::decode::<rasn::types::PrintableString>(bytes) {
        return bytes_to_lossy_string(value.as_bytes());
    }
    if let Ok(value) = rasn::der::decode::<rasn::types::Ia5String>(bytes) {
        return value.to_string();
    }
    if let Ok(value) = rasn::der::decode::<rasn::types::Utf8String>(bytes) {
        return value;
    }

    hex_string(bytes)
}

fn decode_printable_string(bytes: &[u8]) -> String {
    rasn::der::decode::<rasn::types::PrintableString>(bytes)
        .map(|value| bytes_to_lossy_string(value.as_bytes()))
        .unwrap_or_else(|_| hex_string(bytes))
}

fn decode_ia5_string(bytes: &[u8]) -> String {
    rasn::der::decode::<rasn::types::Ia5String>(bytes)
        .map(|value| value.to_string())
        .unwrap_or_else(|_| hex_string(bytes))
}

fn bytes_to_lossy_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn codepoints_to_string(values: impl IntoIterator<Item = u32>) -> String {
    values.into_iter().map(|value| char::from_u32(value).unwrap_or('\u{fffd}')).collect()
}

fn fingerprint_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_string(digest.as_slice())
}

fn hex_string(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>()
}

fn integer_to_serial_string(value: &Integer) -> String {
    let (bytes, len) = value.to_signed_bytes_be();
    let bytes = &bytes.as_ref()[..len];
    if bytes.is_empty() {
        return "00".to_string();
    }
    if value.is_negative() {
        return format!("{value}");
    }
    hex_string(bytes)
}

fn integer_to_u32(value: &Integer) -> Option<u32> {
    let (bytes, len) = value.to_signed_bytes_be();
    let bytes = &bytes.as_ref()[..len];
    if value.is_negative() {
        return None;
    }
    if bytes.is_empty() {
        return Some(0);
    }
    if bytes.len() > 4 {
        return None;
    }
    let mut padded = [0u8; 4];
    let start = padded.len().saturating_sub(bytes.len());
    padded[start..].copy_from_slice(bytes);
    Some(u32::from_be_bytes(padded))
}

fn time_to_unix_ms(time: Time) -> Option<u64> {
    let millis = match time {
        Time::Utc(value) => value.timestamp_millis(),
        Time::General(value) => value.timestamp_millis(),
    };
    u64::try_from(millis).ok()
}

fn time_to_unix_secs(time: Time) -> Option<i64> {
    Some(match time {
        Time::Utc(value) => value.timestamp(),
        Time::General(value) => value.timestamp(),
    })
}

fn find_extension<'a>(extensions: Option<&'a [Extension]>, oid: &str) -> Option<&'a Extension> {
    extensions?.iter().find(|extension| extension.extn_id.to_string() == oid)
}

fn subject_key_identifier(extensions: Option<&[Extension]>) -> Option<String> {
    let extension = find_extension(extensions, OID_EXT_SUBJECT_KEY_IDENTIFIER)?;
    let value: SubjectKeyIdentifier = rasn::der::decode(extension.extn_value.as_ref()).ok()?;
    Some(hex_string(value.as_ref()))
}

fn authority_key_identifier(extensions: Option<&[Extension]>) -> Option<String> {
    let extension = find_extension(extensions, OID_EXT_AUTHORITY_KEY_IDENTIFIER)?;
    let value: AuthorityKeyIdentifier = rasn::der::decode(extension.extn_value.as_ref()).ok()?;
    value.key_identifier.as_ref().map(|identifier| hex_string(identifier.as_ref()))
}

fn subject_alt_dns_names(extensions: Option<&[Extension]>) -> Vec<String> {
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

fn basic_constraints(extensions: Option<&[Extension]>) -> Option<BasicConstraints> {
    let extension = find_extension(extensions, OID_EXT_BASIC_CONSTRAINTS)?;
    rasn::der::decode(extension.extn_value.as_ref()).ok()
}

#[derive(Debug, Clone)]
struct ParsedKeyUsage {
    digital_signature: bool,
    key_encipherment: bool,
    key_agreement: bool,
    key_cert_sign: bool,
    names: Vec<&'static str>,
}

fn key_usage(extensions: Option<&[Extension]>) -> Option<ParsedKeyUsage> {
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

fn describe_key_usage(value: &ParsedKeyUsage) -> String {
    value.names.join(",")
}

fn extended_key_usage(extensions: Option<&[Extension]>) -> Option<Vec<String>> {
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
