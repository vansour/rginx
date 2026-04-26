#![allow(dead_code)]

#[ctor::ctor]
fn install_test_crypto_provider() {
    rginx_http::install_default_crypto_provider();
}

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};

pub const READY_ROUTE_CONFIG: &str = "        LocationConfig(\n            matcher: Exact(\"/-/ready\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ready\\n\"),\n            ),\n        ),\n";

const READY_PATH: &str = "/-/ready";
const READY_BODY: &str = "ready\n";
const DEFAULT_TLS_SERVER_NAME: &str = "localhost";

mod fs_paths;
mod harness;
mod http;
mod response;
mod tls;

#[allow(unused_imports)]
pub use harness::ServerHarness;
#[allow(unused_imports)]
pub use http::{
    HttpChunkRead, apply_tls_placeholders, connect_http_client, read_http_chunk, read_http_head,
    read_http_head_and_pending, reserve_loopback_addr, spawn_scripted_chunked_response_server,
};

use fs_paths::{binary_path, read_optional_log, temp_dir};
use response::{fetch_http_text_response, fetch_https_text_response};
use tls::InsecureServerCertVerifier;
