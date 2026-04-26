//! HTTP/3 listener bootstrap, connection lifecycle, and request/response bridging.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use aws_lc_rs::{hkdf, hmac, rand, rand::SecureRandom};
use bytes::{Buf, Bytes};
use h3::quic::{RecvStream, SendStream};
use h3::server::{Connection as H3Connection, RequestResolver, RequestStream};
use http::{Response, Version};
use http_body_util::BodyExt;
use hyper::body::{Body, Frame, SizeHint};
use quinn::Incoming;
use sha2::{Digest, Sha256};
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};

use rginx_core::{Error, Result};

use crate::client_ip::{ConnectionPeerAddrs, TlsClientIdentity};
use crate::handler::{BoxError, HttpResponse};
use crate::tls::build_http3_server_config;

mod accept_loop;
mod body;
mod close_reason;
mod connection;
mod endpoint;
mod host_key;
mod request;
mod response;
#[cfg(test)]
mod tests;

pub use accept_loop::serve_http3;
pub use endpoint::{bind_http3_endpoint, bind_http3_endpoint_with_socket};

#[cfg(test)]
use close_reason::is_clean_http3_accept_close;
