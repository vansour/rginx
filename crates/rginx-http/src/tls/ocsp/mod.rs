//! OCSP responder discovery, request construction, and response validation.

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

mod der_helpers;
mod discover;
mod nonce;
mod request;
mod signer;
#[cfg(test)]
mod tests;
mod time;
mod validate;

pub(crate) use discover::ocsp_responder_urls_for_certificate;
pub(crate) use request::{
    build_ocsp_request_for_certificate, build_ocsp_request_for_certificate_with_options,
};
pub(crate) use validate::{
    validate_ocsp_response_for_certificate, validate_ocsp_response_for_certificate_with_options,
};

use der_helpers::{
    algorithm_identifier_value_bytes, basic_ocsp_response_type_oid, bit_string_flag,
    build_rasn_ocsp_cert_id_from_chain, certificate_extended_key_usage, certificate_key_usage,
    hex_string, parse_leaf_and_issuer_certificates, signature_bytes, subject_public_key_bytes,
};
use nonce::{build_ocsp_nonce_extension, build_request_nonce, extract_ocsp_nonce};
use signer::validate_basic_ocsp_response_signature;
use time::{certificate_valid_now, generalized_time_from_system_time};
