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

mod decode;
mod extensions;
mod helpers;
mod identity;
mod inspect;
mod name;

pub(crate) use identity::parse_tls_client_identity;
pub(crate) use inspect::inspect_certificate;

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

#[cfg(test)]
mod tests;
