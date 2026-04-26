use std::env;
use std::fs;
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rcgen::{
    BasicConstraints, CertificateParams, CertifiedKey, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair,
};

#[path = "check/basic.rs"]
mod basic;
#[path = "check/helpers.rs"]
mod helpers;
#[path = "check/includes.rs"]
mod includes;
#[path = "check/summary.rs"]
mod summary;
#[path = "check/tls.rs"]
mod tls;

pub(crate) use helpers::*;
