use std::env;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use proptest::prelude::*;
use rasn::types::{BitString, GeneralizedTime, Integer, OctetString};
use rasn_ocsp::{
    BasicOcspResponse as RasnBasicOcspResponse, CertStatus as RasnCertStatus,
    OcspResponse as RasnOcspResponse, OcspResponseStatus as RasnOcspResponseStatus,
    ResponderId as RasnResponderId, ResponseBytes as RasnResponseBytes,
    ResponseData as RasnResponseData, SingleResponse as RasnSingleResponse,
};
use rcgen::{
    BasicConstraints, CertificateParams, CustomExtension, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, PKCS_ED25519, PKCS_RSA_SHA256,
    SigningKey,
};
use rginx_core::ServerCertificateBundle;

use super::*;

mod discovery;
mod nonce;
mod support;
mod validation;

pub(crate) use support::*;
