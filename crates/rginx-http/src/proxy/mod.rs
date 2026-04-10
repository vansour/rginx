use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::future::Future;
use std::io::BufReader;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use bytes::{Bytes, BytesMut};
use http::header::{
    CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST, HeaderMap, HeaderName, HeaderValue,
    PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};
use http::{Method, Request, Response, StatusCode, Uri, Version};
use http_body_util::BodyExt;
use hyper::body::{Body as _, Frame, Incoming, SizeHint};
use hyper::upgrade::OnUpgrade;
use hyper_rustls::{FixedServerNameResolver, HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use pin_project_lite::pin_project;
use rginx_core::{
    ActiveHealthCheck, ConfigSnapshot, Error, ProxyTarget, Upstream, UpstreamPeer,
    UpstreamProtocol, UpstreamTls,
};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tokio::io::copy_bidirectional;
use tokio::time::Instant as TokioInstant;

use crate::client_ip::ClientAddress;
use crate::handler::{
    BoxError, GrpcStatusCode, HttpBody, HttpResponse, full_body, grpc_error_response,
};
use crate::state::SharedState;
use crate::timeout::{GrpcDeadlineBody, IdleTimeoutBody};

mod clients;
mod common;
mod error_mapping;
mod forward;
mod grpc_web;
mod health;
mod request_body;
#[cfg(test)]
mod tests;
mod upgrade;

const MAX_FAILOVER_ATTEMPTS: usize = 2;
const GRPC_CONTENT_TYPE_PREFIX: &str = "application/grpc";
const GRPC_WEB_CONTENT_TYPE_PREFIX: &str = "application/grpc-web";
const GRPC_WEB_TEXT_CONTENT_TYPE_PREFIX: &str = "application/grpc-web-text";
const GRPC_TIMEOUT_HEADER: &str = "grpc-timeout";
const MAX_GRPC_TIMEOUT_DIGITS: usize = 8;

pub(crate) use clients::HealthChangeNotifier;
pub use clients::{ProxyClient, ProxyClients};
pub use forward::{DownstreamRequestContext, DownstreamRequestOptions, forward_request};
pub use health::probe_upstream_peer;
pub use health::{PeerHealthSnapshot, UpstreamHealthSnapshot};

use self::common::*;
pub(crate) use error_mapping::{classify_upstream_tls_failure, upstream_tls_verify_label};
use grpc_web::GrpcWebMode;
