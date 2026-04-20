pub(super) use std::collections::{HashMap, HashSet};
pub(super) use std::sync::Arc;
pub(super) use std::time::Duration;

pub(super) use base64::Engine as _;
pub(super) use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
pub(super) use bytes::{Bytes, BytesMut};
pub(super) use http::header::{
    CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST, HeaderMap, HeaderName, HeaderValue,
    PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
};
pub(super) use http::{Method, Request, Response, StatusCode, Uri, Version};
pub(super) use http_body_util::BodyExt;
pub(super) use hyper::body::{Body as _, Frame, SizeHint};
pub(super) use hyper::upgrade::OnUpgrade;
pub(super) use hyper_rustls::{
    FixedServerNameResolver, HttpsConnector, HttpsConnectorBuilder, ResolveServerName,
};
pub(super) use hyper_util::client::legacy::Client;
pub(super) use hyper_util::client::legacy::connect::HttpConnector;
pub(super) use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
pub(super) use pin_project_lite::pin_project;
pub(super) use rginx_core::{
    ActiveHealthCheck, ConfigSnapshot, Error, ProxyTarget, RouteBufferingPolicy, Upstream,
    UpstreamPeer, UpstreamProtocol, UpstreamTls,
};
pub(super) use rustls::pki_types::ServerName;
pub(super) use tokio::io::copy_bidirectional;
pub(super) use tokio::time::Instant as TokioInstant;

use crate::client_ip::ClientAddress;
use crate::handler::{
    BoxError, GrpcStatusCode, HttpBody, HttpResponse, full_body, grpc_error_response,
};
use crate::state::SharedState;
use crate::timeout::{GrpcDeadlineBody, IdleTimeoutBody, MaxBytesBody, RequestBodyLimitError};

mod clients;
mod common;
mod error_mapping;
mod forward;
mod grpc_web;
mod health;
mod request_body;
mod resolver;
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
pub use clients::ProxyClients;
pub use forward::{DownstreamRequestContext, DownstreamRequestOptions, forward_request};
pub use health::probe_upstream_peer;
pub use health::{PeerHealthSnapshot, UpstreamHealthSnapshot};

use self::common::*;
pub(crate) use error_mapping::{classify_upstream_tls_failure, upstream_tls_verify_label};
pub(super) use grpc_web::GrpcWebMode;
pub(crate) use resolver::{
    ResolvedUpstreamPeer, UpstreamResolver, UpstreamResolverRuntimeSnapshot,
};
