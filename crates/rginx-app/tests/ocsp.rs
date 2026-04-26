#![cfg(unix)]

use std::env;
use std::fs;
use std::io::Write;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};
use rasn::types::{BitString, GeneralizedTime, Integer, ObjectIdentifier, OctetString};
use rasn_ocsp::{
    BasicOcspResponse as RasnBasicOcspResponse, CertId as RasnCertId, CertStatus as RasnCertStatus,
    OcspRequest as RasnOcspRequest, OcspResponse as RasnOcspResponse,
    OcspResponseStatus as RasnOcspResponseStatus, ResponderId as RasnResponderId,
    ResponseBytes as RasnResponseBytes, ResponseData as RasnResponseData,
    SingleResponse as RasnSingleResponse,
};
use rcgen::{
    BasicConstraints, CertificateParams, CustomExtension, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose, PKCS_ECDSA_P256_SHA256, PKCS_ED25519, PKCS_RSA_SHA256,
    SigningKey,
};
use sha1::Digest;

mod support;

pub(crate) use support::{
    READY_ROUTE_CONFIG, ServerHarness, read_http_head, reserve_loopback_addr,
};

#[path = "ocsp/cache.rs"]
mod cache;
#[path = "ocsp/helpers.rs"]
mod helpers;
#[path = "ocsp/refresh.rs"]
mod refresh;

pub(crate) use helpers::*;
