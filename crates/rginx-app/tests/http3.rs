use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::{Buf, Bytes, BytesMut};
use flate2::read::GzDecoder;
use h3::client;
use http_body_util::Empty;
use hyper::http::{Request, StatusCode};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use quinn::crypto::rustls::QuicClientConfig;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
};
use rustls::pki_types::{CertificateDer, pem::PemObject};
use rustls::{ClientConfig, RootCertStore};

mod support;

pub(crate) use support::{READY_ROUTE_CONFIG, ServerHarness, reserve_loopback_addr};

#[path = "http3/basic.rs"]
mod basic;
#[path = "http3/early_data.rs"]
mod early_data;
#[path = "http3/helpers/client.rs"]
mod helpers_client;
#[path = "http3/helpers/config.rs"]
mod helpers_config;
#[path = "http3/helpers/fixtures.rs"]
mod helpers_fixtures;
#[path = "http3/mtls.rs"]
mod mtls;
#[path = "http3/policy.rs"]
mod policy;
#[path = "http3/retry.rs"]
mod retry;

pub(crate) use helpers_client::*;
pub(crate) use helpers_config::*;
pub(crate) use helpers_fixtures::*;
