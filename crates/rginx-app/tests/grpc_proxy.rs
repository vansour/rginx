#[allow(unused_imports)]
use std::convert::Infallible;
#[allow(unused_imports)]
use std::fs;
#[allow(unused_imports)]
use std::future::Future;
#[allow(unused_imports)]
use std::net::SocketAddr;
#[allow(unused_imports)]
use std::path::{Path, PathBuf};
#[allow(unused_imports)]
use std::pin::Pin;
#[allow(unused_imports)]
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
#[allow(unused_imports)]
use std::sync::{Arc, Mutex};
#[allow(unused_imports)]
use std::task::{Context, Poll};
#[allow(unused_imports)]
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[allow(unused_imports)]
use base64::Engine as _;
#[allow(unused_imports)]
use base64::engine::general_purpose::STANDARD;
#[allow(unused_imports)]
use bytes::{Bytes, BytesMut};
#[allow(unused_imports)]
use http_body_util::{BodyExt, Empty, Full};
#[allow(unused_imports)]
use hyper::body::{Body, Frame, Incoming, SizeHint};
#[allow(unused_imports)]
use hyper::http::HeaderMap;
#[allow(unused_imports)]
use hyper::http::header::{CONTENT_TYPE, HeaderName, HeaderValue, TE};
#[allow(unused_imports)]
use hyper::server::conn::http2;
#[allow(unused_imports)]
use hyper::service::service_fn;
#[allow(unused_imports)]
use hyper::{Request, Response, StatusCode, Version};
#[allow(unused_imports)]
use hyper_rustls::HttpsConnectorBuilder;
#[allow(unused_imports)]
use hyper_util::client::legacy::Client;
#[allow(unused_imports)]
use hyper_util::rt::{TokioExecutor, TokioIo};
#[allow(unused_imports)]
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
#[allow(unused_imports)]
use rustls::pki_types::pem::PemObject;
#[allow(unused_imports)]
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
#[allow(unused_imports)]
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
#[allow(unused_imports)]
use tokio::sync::oneshot;
#[allow(unused_imports)]
use tokio::task::JoinHandle;
#[allow(unused_imports)]
use tokio_rustls::TlsAcceptor;

mod support;

pub(crate) use support::{
    READY_ROUTE_CONFIG, ServerHarness, apply_tls_placeholders, reserve_loopback_addr,
};

const TEST_SERVER_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIDCTCCAfGgAwIBAgIUE+LKmhgfKie/YU/anMKv+Xgr5dYwDQYJKoZIhvcNAQEL\nBQAwFDESMBAGA1UEAwwJbG9jYWxob3N0MB4XDTI2MDMyMDE1MzIzMloXDTI2MDMy\nMTE1MzIzMlowFDESMBAGA1UEAwwJbG9jYWxob3N0MIIBIjANBgkqhkiG9w0BAQEF\nAAOCAQ8AMIIBCgKCAQEAvxn1IYqOORs2Ys/6Ou54G3alu+wZOeGkPy/ZLYUuO0pK\nh1WgvPvwGF3w3XZdEPhB0JXhqwqoz60SwGQJtEM9GGRHVnBV+BeE/4L1XO4H6Gz5\npMKFaCcJPwO4IrspjffpKQ217K9l9vbjK31tJKwOGaQ//icyzF13xuUvZms67PNc\nBqhZQchld9s90InnL3fCS+J58s9pjE0qlTr7bodvOXaYBxboDlBh4YV7PW/wjwBo\ngUwcbiJvtrRnY7ZlRi/C/bZUTGJ5kO7vSlAgMh2KL1DyY2Ws06n5KUNgpAuIjmew\nMtuYJ9H2xgRMrMjgWSD8N/RRFut4xnpm7jlRepzvwwIDAQABo1MwUTAdBgNVHQ4E\nFgQUIezWZPz8VZj6n2znyGWv76RsGMswHwYDVR0jBBgwFoAUIezWZPz8VZj6n2zn\nyGWv76RsGMswDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAbngq\np7KT2JaXL8BYQGThBZwRODtqv/jXwc34zE3DPPRb1F3i8/odH7+9ZLse35Hj0/gp\nqFQ0DNdOuNlrbrvny208P1OcBe2hYWOSsRGyhZpM5Ai+DkuHheZfhNKvWKdbFn8+\nyfeyN3orSsin9QG0Yx3eqtO/1/6D5TtLsnY2/yPV/j0pv2GCCuB0kcKfygOQTYW6\nJrmYzeFeR/bnQM/lOM49leURdgC/x7tveNG7KRvD0X85M9iuT9/0+VSu6yAkcEi5\nx23C/Chzu7FFVxwZRHD+RshbV4QTPewhi17EJwroMYFpjGUHJVUfzo6W6bsWqA59\nCiiHI87NdBZv4JUCOQ==\n-----END CERTIFICATE-----\n";
const TEST_SERVER_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC/GfUhio45GzZi\nz/o67ngbdqW77Bk54aQ/L9kthS47SkqHVaC8+/AYXfDddl0Q+EHQleGrCqjPrRLA\nZAm0Qz0YZEdWcFX4F4T/gvVc7gfobPmkwoVoJwk/A7giuymN9+kpDbXsr2X29uMr\nfW0krA4ZpD/+JzLMXXfG5S9mazrs81wGqFlByGV32z3Qiecvd8JL4nnyz2mMTSqV\nOvtuh285dpgHFugOUGHhhXs9b/CPAGiBTBxuIm+2tGdjtmVGL8L9tlRMYnmQ7u9K\nUCAyHYovUPJjZazTqfkpQ2CkC4iOZ7Ay25gn0fbGBEysyOBZIPw39FEW63jGembu\nOVF6nO/DAgMBAAECggEAKLC7v80TVHiFX4veQZ8WRu7AAmAWzPrNMMEc8rLZcblz\nXhau956DdITILTevQFZEGUhYuUU3RaUaCYojgNUSVLfBctfPjlhfstItMYDjgSt3\nCox6wH8TWm4NzqNgiUCgzmODeaatROUz4MY/r5/NDsuo7pJlIBvEzb5uFdY+QUZ/\nR5gHRiD2Q3wCODe8zQRfTZGo7jCimAuWTLurWZl6ax/4TjWbXCD6DTuUo81cW3vy\nne6tEetHcABRO7uDoBYXk12pCgqFZzjLMnKJjQM+OYnSj6DoWjOu1drT5YyRLGDj\nfzN8V0aKRkOYoZ5QZOua8pByOyQElJnM16vkPtHgPQKBgQD6SOUNWEghvYIGM/lx\nc22/zjvDjeaGC3qSmlpQYN5MGuDoszeDBZ+rMTmHqJ9FcHYkLQnUI7ZkHhRGt/wQ\n/w3CroJjPBgKk+ipy2cBHSI+z+U20xjYzE8hxArWbXG1G4rDt5AIz68IQPsfkVND\nktkDABDaU+KwBPx8fjeeqtRQxQKBgQDDdxdLB1XcfZMX0KEP5RfA8ar1nW41TUAl\nTCOLaXIQbHZ0BeW7USE9mK8OKnVALZGJ+rpxvYFPZ5MWxchpb/cuIwXjLoN6uZVb\nfx4Hho+2iCfhcEKzs8XZW48duKIfhx13BiILLf/YaHAWFs9UfVcQog4Qx03guyMr\n7k9bFuy25wKBgQDpE48zAT6TJS775dTrAQp4b28aan/93pyz/8gRSFRb3UALlDIi\n8s7BluKzYaWI/fUXNVYM14EX9Sb+wIGdtlezL94+2Yyt9RXbYY8361Cj2+jiSG3A\nH2ulzzIkg+E7Pj3Yi443lmiysAjsWeKHcC5l697F4w6cytfye3wCZ6W23QKBgQC0\n9tX+5aytdSkwnDvxXlVOka+ItBcri/i+Ty59TMOIxxInuqoFcUhIIcq4X8CsCUQ8\nLYBd+2fznt3D8JrqWvnKoiw6N38MqTLJQfgIWaFGCep6QhfPDbo30RfAGYcnj01N\nO8Va+lxq+84B9V5AR8bKpG5HRG4qiLc4XerkV2YSswKBgDt9eerSBZyLVwfku25Y\nfrh+nEjUZy81LdlpJmu/bfa2FfItzBqDZPskkJJW9ON82z/ejGFbsU48RF7PJUMr\nGimE33QeTDToGozHCq0QOd0SMfsVkOQR+EROdmY52UIYAYgQUfI1FQ9lLsw10wlQ\nD11SHTL7b9pefBWfW73I7ttV\n-----END PRIVATE KEY-----\n";
const GRPC_METHOD_PATH: &str = "/grpc.health.v1.Health/Check";
const APP_GRPC_METHOD_PATH: &str = "/demo.Test/Ping";
const GRPC_REQUEST_FRAME: &[u8] = b"\x00\x00\x00\x00\x02hi";
const GRPC_RESPONSE_FRAME: &[u8] = b"\x00\x00\x00\x00\x02ok";

#[derive(Debug)]
struct ObservedRequest {
    method: String,
    version: Version,
    path: String,
    alpn_protocol: Option<String>,
    content_type: Option<String>,
    grpc_timeout: Option<String>,
    te: Option<String>,
    body: Bytes,
    trailers: Option<HeaderMap>,
}

#[path = "grpc_proxy/basic.rs"]
mod basic;
#[path = "grpc_proxy/lifecycle.rs"]
mod lifecycle;
#[path = "grpc_proxy/timeout.rs"]
mod timeout;

#[path = "grpc_proxy/helpers/body.rs"]
mod helpers_body;
#[path = "grpc_proxy/helpers/config.rs"]
mod helpers_config;
#[path = "grpc_proxy/helpers/grpc_web.rs"]
mod helpers_grpc_web;
#[path = "grpc_proxy/helpers/server.rs"]
mod helpers_server;
#[path = "grpc_proxy/helpers/tls.rs"]
mod helpers_tls;
#[path = "grpc_proxy/helpers/upstream.rs"]
mod helpers_upstream;

pub(crate) use helpers_body::*;
pub(crate) use helpers_config::*;
pub(crate) use helpers_grpc_web::*;
pub(crate) use helpers_server::*;
pub(crate) use helpers_tls::*;
pub(crate) use helpers_upstream::*;
