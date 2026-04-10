#![allow(unused_imports)]

use std::convert::Infallible;
use std::fs::{self, File};
use std::future::Future;
use std::io::BufReader;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use bytes::{Bytes, BytesMut};
use http_body_util::{BodyExt, Empty, Full};
use hyper::body::{Body, Frame, Incoming, SizeHint};
use hyper::http::HeaderMap;
use hyper::http::header::{CONTENT_TYPE, HeaderName, HeaderValue, TE};
use hyper::server::conn::http2;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode, Version};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_rustls::TlsAcceptor;

mod support;

use support::{READY_ROUTE_CONFIG, ServerHarness, apply_tls_placeholders, reserve_loopback_addr};

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

async fn spawn_grpc_upstream()
-> (SocketAddr, oneshot::Receiver<ObservedRequest>, JoinHandle<()>, PathBuf) {
    spawn_grpc_upstream_with_mode(UpstreamResponseMode::Immediate).await
}

async fn spawn_grpc_upstream_with_response_delay(
    response_delay: Duration,
) -> (SocketAddr, oneshot::Receiver<ObservedRequest>, JoinHandle<()>, PathBuf) {
    spawn_grpc_upstream_with_mode(UpstreamResponseMode::DelayHeaders(response_delay)).await
}

async fn spawn_grpc_upstream_with_body_delay(
    body_delay: Duration,
) -> (SocketAddr, oneshot::Receiver<ObservedRequest>, JoinHandle<()>, PathBuf) {
    spawn_grpc_upstream_with_mode(UpstreamResponseMode::DelayBody(body_delay)).await
}

async fn spawn_grpc_upstream_with_dynamic_health(
    health_status: Arc<AtomicU8>,
) -> (SocketAddr, oneshot::Sender<()>, JoinHandle<()>, PathBuf) {
    let temp_dir = temp_dir("rginx-grpc-health-upstream");
    fs::create_dir_all(&temp_dir).expect("upstream temp dir should be created");
    let cert_path = temp_dir.join("upstream.crt");
    let key_path = temp_dir.join("upstream.key");
    fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("upstream cert should be written");
    fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("upstream key should be written");

    let certs = load_certs(&cert_path);
    let key = load_private_key(&key_path);
    let mut tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("test upstream TLS config should build");
    tls_config.alpn_protocols = vec![b"h2".to_vec()];
    let tls_acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("upstream gRPC listener should bind");
    let listen_addr = listener.local_addr().expect("upstream gRPC addr should be available");
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();

    let task = tokio::spawn(async move {
        loop {
            let stream = tokio::select! {
                _ = &mut shutdown_rx => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else {
                        break;
                    };
                    stream
                }
            };
            let tls_stream =
                tls_acceptor.accept(stream).await.expect("upstream TLS handshake should work");

            let health_status = health_status.clone();
            let service = service_fn(move |request: Request<Incoming>| {
                let health_status = health_status.clone();

                async move {
                    let path = request.uri().path().to_string();
                    let response = if path == GRPC_METHOD_PATH {
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(CONTENT_TYPE, "application/grpc")
                            .header("grpc-status", "0")
                            .body(EitherGrpcResponseBody::Full(Full::new(
                                grpc_health_response_frame(health_status.load(Ordering::Relaxed)),
                            )))
                            .expect("gRPC health response should build")
                    } else if path == APP_GRPC_METHOD_PATH {
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(CONTENT_TYPE, "application/grpc")
                            .header("grpc-status", "0")
                            .body(EitherGrpcResponseBody::Full(Full::new(Bytes::from_static(
                                GRPC_RESPONSE_FRAME,
                            ))))
                            .expect("upstream gRPC response should build")
                    } else {
                        Response::builder()
                            .status(StatusCode::OK)
                            .header(CONTENT_TYPE, "application/grpc")
                            .body(EitherGrpcResponseBody::Immediate(GrpcResponseBody::new()))
                            .expect("upstream gRPC response should build")
                    };

                    Ok::<_, Infallible>(response)
                }
            });

            let _ = http2::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(tls_stream), service)
                .await;
        }
    });

    (listen_addr, shutdown_tx, task, temp_dir)
}

async fn spawn_cancellable_grpc_upstream()
-> (SocketAddr, oneshot::Receiver<()>, JoinHandle<()>, PathBuf) {
    let temp_dir = temp_dir("rginx-grpc-cancel-upstream");
    fs::create_dir_all(&temp_dir).expect("upstream temp dir should be created");
    let cert_path = temp_dir.join("upstream.crt");
    let key_path = temp_dir.join("upstream.key");
    fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("upstream cert should be written");
    fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("upstream key should be written");

    let certs = load_certs(&cert_path);
    let key = load_private_key(&key_path);
    let mut tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("test upstream TLS config should build");
    tls_config.alpn_protocols = vec![b"h2".to_vec()];
    let tls_acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("upstream gRPC listener should bind");
    let listen_addr = listener.local_addr().expect("upstream gRPC addr should be available");
    let (cancelled_tx, cancelled_rx) = oneshot::channel();
    let cancelled_tx = Arc::new(Mutex::new(Some(cancelled_tx)));

    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("upstream listener should accept");
        let tls_stream =
            tls_acceptor.accept(stream).await.expect("upstream TLS handshake should work");

        let service = service_fn(move |request: Request<Incoming>| {
            let cancelled_tx = cancelled_tx.clone();
            async move {
                assert_eq!(request.uri().path(), APP_GRPC_METHOD_PATH);
                Ok::<_, Infallible>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(CONTENT_TYPE, "application/grpc")
                        .body(EitherGrpcResponseBody::Cancellable(
                            CancellableGrpcResponseBody::new(cancelled_tx),
                        ))
                        .expect("cancellable gRPC response should build"),
                )
            }
        });

        http2::Builder::new(TokioExecutor::new())
            .serve_connection(TokioIo::new(tls_stream), service)
            .await
            .expect("upstream gRPC h2 connection should complete");
    });

    (listen_addr, cancelled_rx, task, temp_dir)
}

async fn spawn_grpc_upstream_with_mode(
    mode: UpstreamResponseMode,
) -> (SocketAddr, oneshot::Receiver<ObservedRequest>, JoinHandle<()>, PathBuf) {
    let temp_dir = temp_dir("rginx-grpc-upstream");
    fs::create_dir_all(&temp_dir).expect("upstream temp dir should be created");
    let cert_path = temp_dir.join("upstream.crt");
    let key_path = temp_dir.join("upstream.key");
    fs::write(&cert_path, TEST_SERVER_CERT_PEM).expect("upstream cert should be written");
    fs::write(&key_path, TEST_SERVER_KEY_PEM).expect("upstream key should be written");

    let certs = load_certs(&cert_path);
    let key = load_private_key(&key_path);
    let mut tls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("test upstream TLS config should build");
    tls_config.alpn_protocols = vec![b"h2".to_vec()];
    let tls_acceptor = TlsAcceptor::from(Arc::new(tls_config));

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("upstream gRPC listener should bind");
    let listen_addr = listener.local_addr().expect("upstream gRPC addr should be available");
    let (observed_tx, observed_rx) = oneshot::channel();
    let observed_tx = Arc::new(Mutex::new(Some(observed_tx)));

    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("upstream listener should accept");
        let tls_stream =
            tls_acceptor.accept(stream).await.expect("upstream TLS handshake should work");
        let alpn_protocol = tls_stream
            .get_ref()
            .1
            .alpn_protocol()
            .map(|protocol| String::from_utf8_lossy(protocol).into_owned());

        let service = service_fn(move |request: Request<Incoming>| {
            let observed_tx = observed_tx.clone();
            let alpn_protocol = alpn_protocol.clone();

            async move {
                let (parts, body) = request.into_parts();
                let (body_bytes, trailers) = read_body_and_trailers(body).await;

                if let Some(sender) =
                    observed_tx.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()
                {
                    let _ = sender.send(ObservedRequest {
                        method: parts.method.as_str().to_string(),
                        version: parts.version,
                        path: parts.uri.path().to_string(),
                        alpn_protocol,
                        content_type: parts
                            .headers
                            .get(CONTENT_TYPE)
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_string),
                        grpc_timeout: parts
                            .headers
                            .get("grpc-timeout")
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_string),
                        te: parts
                            .headers
                            .get(TE)
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_string),
                        body: body_bytes.freeze(),
                        trailers,
                    });
                }

                if let UpstreamResponseMode::DelayHeaders(response_delay) = mode
                    && !response_delay.is_zero()
                {
                    tokio::time::sleep(response_delay).await;
                }

                let body = match mode {
                    UpstreamResponseMode::Immediate | UpstreamResponseMode::DelayHeaders(_) => {
                        EitherGrpcResponseBody::Immediate(GrpcResponseBody::new())
                    }
                    UpstreamResponseMode::DelayBody(body_delay) => {
                        EitherGrpcResponseBody::Delayed(DelayedGrpcResponseBody::new(body_delay))
                    }
                };

                Ok::<_, Infallible>(
                    Response::builder()
                        .status(StatusCode::OK)
                        .header(CONTENT_TYPE, "application/grpc")
                        .body(body)
                        .expect("upstream gRPC response should build"),
                )
            }
        });

        http2::Builder::new(TokioExecutor::new())
            .serve_connection(TokioIo::new(tls_stream), service)
            .await
            .expect("upstream gRPC h2 connection should complete");
    });

    (listen_addr, observed_rx, task, temp_dir)
}

async fn read_body_and_trailers(body: Incoming) -> (BytesMut, Option<HeaderMap>) {
    let mut body = body;
    let mut bytes = BytesMut::new();
    let mut trailers = None;

    while let Some(frame) = body.frame().await {
        let frame = frame.expect("response frame should succeed");
        match frame.into_data() {
            Ok(data) => bytes.extend_from_slice(&data),
            Err(frame) => match frame.into_trailers() {
                Ok(frame_trailers) => trailers = Some(frame_trailers),
                Err(_) => panic!("unexpected non-data, non-trailers frame"),
            },
        }
    }

    (bytes, trailers)
}

fn decode_grpc_web_response(bytes: &[u8]) -> (Vec<Bytes>, HeaderMap) {
    let mut offset = 0usize;
    let mut frames = Vec::new();
    let mut trailers = HeaderMap::new();

    while offset < bytes.len() {
        assert!(
            bytes.len().saturating_sub(offset) >= 5,
            "grpc-web frame should include a 5-byte header"
        );
        let flags = bytes[offset];
        let len = u32::from_be_bytes([
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
            bytes[offset + 4],
        ]) as usize;
        offset += 5;
        assert!(
            bytes.len().saturating_sub(offset) >= len,
            "grpc-web frame payload should be fully present"
        );
        let payload = &bytes[offset..offset + len];
        offset += len;

        if (flags & 0x80) != 0 {
            for line in payload.split(|byte| *byte == b'\n') {
                let line = line.strip_suffix(b"\r").unwrap_or(line);
                if line.is_empty() {
                    continue;
                }

                let Some(separator) = line.iter().position(|byte| *byte == b':') else {
                    panic!("grpc-web trailer line should contain ':'");
                };
                let (name, value) = line.split_at(separator);
                let value = &value[1..];
                let name =
                    std::str::from_utf8(name).expect("grpc-web trailer name should be utf-8");
                let value = std::str::from_utf8(value)
                    .expect("grpc-web trailer value should be utf-8")
                    .trim();
                trailers.insert(
                    name.parse::<HeaderName>().expect("grpc-web trailer name should be valid"),
                    HeaderValue::from_str(value).expect("grpc-web trailer value should be valid"),
                );
            }
        } else {
            frames.push(Bytes::copy_from_slice(payload));
        }
    }

    (frames, trailers)
}

fn encode_grpc_web_text_payload(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

fn grpc_web_request_with_trailers() -> Bytes {
    let mut request = BytesMut::from(GRPC_REQUEST_FRAME);
    request.extend_from_slice(grpc_web_trailer_frame().as_ref());
    request.freeze()
}

fn grpc_web_trailer_frame() -> Bytes {
    let block = b"x-client-trailer: sent\r\nx-request-checksum: abc123\r\n";
    let mut frame = Vec::with_capacity(5 + block.len());
    frame.push(0x80);
    frame.extend_from_slice(&(block.len() as u32).to_be_bytes());
    frame.extend_from_slice(block);
    Bytes::from(frame)
}

fn grpc_health_response_frame(serving_status: u8) -> Bytes {
    Bytes::from(vec![0x00, 0x00, 0x00, 0x00, 0x02, 0x08, serving_status])
}

fn decode_grpc_web_text_payload(bytes: &[u8]) -> Vec<u8> {
    let filtered =
        bytes.iter().copied().filter(|byte| !byte.is_ascii_whitespace()).collect::<Vec<_>>();
    let mut decoded = Vec::new();

    for quantum in filtered.chunks_exact(4) {
        let chunk = STANDARD.decode(quantum).expect("grpc-web-text payload should be valid base64");
        decoded.extend_from_slice(&chunk);
    }

    assert_eq!(
        filtered.len() % 4,
        0,
        "grpc-web-text payload should end on a base64 quantum boundary"
    );
    decoded
}

struct GrpcResponseBody {
    state: u8,
}

impl GrpcResponseBody {
    fn new() -> Self {
        Self { state: 0 }
    }
}

impl Body for GrpcResponseBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        match this.state {
            0 => {
                this.state = 1;
                Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(GRPC_RESPONSE_FRAME)))))
            }
            1 => {
                this.state = 2;
                let mut trailers = HeaderMap::new();
                trailers.insert("grpc-status", HeaderValue::from_static("0"));
                trailers.insert("grpc-message", HeaderValue::from_static("ok"));
                Poll::Ready(Some(Ok(Frame::trailers(trailers))))
            }
            _ => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.state >= 2
    }

    fn size_hint(&self) -> SizeHint {
        let mut hint = SizeHint::new();
        hint.set_exact(GRPC_RESPONSE_FRAME.len() as u64);
        hint
    }
}

#[derive(Clone, Copy)]
enum UpstreamResponseMode {
    Immediate,
    DelayHeaders(Duration),
    DelayBody(Duration),
}

struct DelayedGrpcResponseBody {
    state: u8,
    delay: Pin<Box<tokio::time::Sleep>>,
}

impl DelayedGrpcResponseBody {
    fn new(delay: Duration) -> Self {
        Self { state: 0, delay: Box::pin(tokio::time::sleep(delay)) }
    }
}

enum EitherGrpcResponseBody {
    Immediate(GrpcResponseBody),
    Delayed(DelayedGrpcResponseBody),
    Cancellable(CancellableGrpcResponseBody),
    Full(Full<Bytes>),
}

impl Body for DelayedGrpcResponseBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        match this.state {
            0 => match this.delay.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    this.state = 1;
                    Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(GRPC_RESPONSE_FRAME)))))
                }
                Poll::Pending => Poll::Pending,
            },
            1 => {
                this.state = 2;
                let mut trailers = HeaderMap::new();
                trailers.insert("grpc-status", HeaderValue::from_static("0"));
                trailers.insert("grpc-message", HeaderValue::from_static("ok"));
                Poll::Ready(Some(Ok(Frame::trailers(trailers))))
            }
            _ => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.state >= 2
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

impl Body for EitherGrpcResponseBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.get_mut() {
            Self::Immediate(body) => Pin::new(body).poll_frame(cx),
            Self::Delayed(body) => Pin::new(body).poll_frame(cx),
            Self::Cancellable(body) => Pin::new(body).poll_frame(cx),
            Self::Full(body) => Pin::new(body).poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Self::Immediate(body) => body.is_end_stream(),
            Self::Delayed(body) => body.is_end_stream(),
            Self::Cancellable(body) => body.is_end_stream(),
            Self::Full(body) => body.is_end_stream(),
        }
    }

    fn size_hint(&self) -> SizeHint {
        match self {
            Self::Immediate(body) => body.size_hint(),
            Self::Delayed(body) => body.size_hint(),
            Self::Cancellable(body) => body.size_hint(),
            Self::Full(body) => body.size_hint(),
        }
    }
}

struct CancellableGrpcResponseBody {
    state: u8,
    delay: Pin<Box<tokio::time::Sleep>>,
    cancelled_tx: Option<Arc<Mutex<Option<oneshot::Sender<()>>>>>,
}

impl CancellableGrpcResponseBody {
    fn new(cancelled_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>) -> Self {
        Self {
            state: 0,
            delay: Box::pin(tokio::time::sleep(Duration::from_secs(30))),
            cancelled_tx: Some(cancelled_tx),
        }
    }
}

impl Drop for CancellableGrpcResponseBody {
    fn drop(&mut self) {
        if let Some(cancelled_tx) = self.cancelled_tx.take()
            && let Some(sender) =
                cancelled_tx.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take()
        {
            let _ = sender.send(());
        }
    }
}

impl Body for CancellableGrpcResponseBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        match this.state {
            0 => {
                this.state = 1;
                Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(GRPC_RESPONSE_FRAME)))))
            }
            1 => match this.delay.as_mut().poll(cx) {
                Poll::Ready(()) => {
                    this.state = 2;
                    let mut trailers = HeaderMap::new();
                    trailers.insert("grpc-status", HeaderValue::from_static("0"));
                    trailers.insert("grpc-message", HeaderValue::from_static("ok"));
                    Poll::Ready(Some(Ok(Frame::trailers(trailers))))
                }
                Poll::Pending => Poll::Pending,
            },
            _ => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.state >= 2
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

struct GrpcRequestBody {
    state: u8,
}

impl GrpcRequestBody {
    fn new() -> Self {
        Self { state: 0 }
    }
}

impl Body for GrpcRequestBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        match this.state {
            0 => {
                this.state = 1;
                Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(GRPC_REQUEST_FRAME)))))
            }
            1 => {
                this.state = 2;
                let mut trailers = HeaderMap::new();
                trailers.insert("x-client-trailer", HeaderValue::from_static("sent"));
                trailers.insert("x-request-checksum", HeaderValue::from_static("abc123"));
                Poll::Ready(Some(Ok(Frame::trailers(trailers))))
            }
            _ => Poll::Ready(None),
        }
    }

    fn is_end_stream(&self) -> bool {
        self.state >= 2
    }

    fn size_hint(&self) -> SizeHint {
        let mut hint = SizeHint::new();
        hint.set_exact(GRPC_REQUEST_FRAME.len() as u64);
        hint
    }
}

struct TestServer {
    inner: ServerHarness,
}

impl TestServer {
    fn spawn(listen_addr: SocketAddr, config: String) -> Self {
        let _ = listen_addr;
        Self {
            inner: ServerHarness::spawn_with_tls(
                "rginx-grpc-proxy",
                TEST_SERVER_CERT_PEM,
                TEST_SERVER_KEY_PEM,
                |_, cert_path, key_path| apply_tls_placeholders(config, cert_path, key_path),
            ),
        }
    }

    fn wait_for_http_ready(&mut self, listen_addr: SocketAddr, timeout: Duration) {
        self.inner.wait_for_http_ready(listen_addr, timeout);
    }

    fn wait_for_https_ready(&mut self, listen_addr: SocketAddr, timeout: Duration) {
        self.inner.wait_for_https_ready(listen_addr, timeout);
    }

    fn shutdown_and_wait(&mut self, timeout: Duration) {
        self.inner.shutdown_and_wait(timeout);
    }
}

fn tls_proxy_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    tls_proxy_config_with_request_timeout(listen_addr, upstream_addr, None)
}

fn tls_proxy_config_with_request_timeout(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    request_timeout_secs: Option<u64>,
) -> String {
    let request_timeout_secs = request_timeout_secs
        .map(|secs| format!("            request_timeout_secs: Some({secs}),\n"))
        .unwrap_or_default();
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        tls: Some(ServerTlsConfig(\n            cert_path: \"__CERT_PATH__\",\n            key_path: \"__KEY_PATH__\",\n        )),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n{request_timeout_secs}        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact({GRPC_METHOD_PATH:?}),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n        LocationConfig(\n            matcher: Exact({APP_GRPC_METHOD_PATH:?}),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        request_timeout_secs = request_timeout_secs,
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn plain_proxy_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    plain_proxy_config_with_request_timeout(listen_addr, upstream_addr, None)
}

fn plain_proxy_config_with_server_extra(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    server_extra: &str,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n{}    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact({GRPC_METHOD_PATH:?}),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        server_extra,
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn plain_proxy_config_with_request_timeout(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    request_timeout_secs: Option<u64>,
) -> String {
    let request_timeout_secs = request_timeout_secs
        .map(|secs| format!("            request_timeout_secs: Some({secs}),\n"))
        .unwrap_or_default();
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n{request_timeout_secs}        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact({GRPC_METHOD_PATH:?}),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        request_timeout_secs = request_timeout_secs,
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn plain_proxy_config_with_grpc_health_check(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n            health_check_grpc_service: Some(\"grpc.health.v1.Health\"),\n            health_check_interval_secs: Some(1),\n            health_check_timeout_secs: Some(1),\n            healthy_successes_required: Some(2),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact({APP_GRPC_METHOD_PATH:?}),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn plain_proxy_config_with_access_log(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        access_log_format: Some(\"ACCESS reqid=$request_id grpc=$grpc_protocol svc=$grpc_service rpc=$grpc_method grpc_status=$grpc_status grpc_message=\\\"$grpc_message\\\" route=$route\"),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact({APP_GRPC_METHOD_PATH:?}),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn plain_proxy_config_with_request_timeout_and_access_log(
    listen_addr: SocketAddr,
    upstream_addr: SocketAddr,
    request_timeout_secs: Option<u64>,
) -> String {
    let request_timeout_secs = request_timeout_secs
        .map(|secs| format!("            request_timeout_secs: Some({secs}),\n"))
        .unwrap_or_default();
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        access_log_format: Some(\"ACCESS reqid=$request_id grpc=$grpc_protocol svc=$grpc_service rpc=$grpc_method grpc_status=$grpc_status grpc_message=\\\"$grpc_message\\\" route=$route\"),\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n{request_timeout_secs}        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact({GRPC_METHOD_PATH:?}),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", upstream_addr.port()),
        request_timeout_secs = request_timeout_secs,
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn https_h2_connector()
-> hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector> {
    HttpsConnectorBuilder::new()
        .with_tls_config(
            ClientConfig::builder()
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(InsecureServerCertVerifier::new()))
                .with_no_client_auth(),
        )
        .https_only()
        .enable_http2()
        .build()
}

async fn wait_for_log_contains(server: &TestServer, timeout: Duration, needle: &str) {
    let deadline = Instant::now() + timeout;
    let mut last_logs = String::new();

    while Instant::now() < deadline {
        let logs = server.inner.combined_output();
        if logs.contains(needle) {
            return;
        }
        last_logs = logs;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!("expected log line containing `{needle}`, got:\n{last_logs}");
}

fn tls_unmatched_grpc_config(listen_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        tls: Some(ServerTlsConfig(\n            cert_path: \"__CERT_PATH__\",\n            key_path: \"__KEY_PATH__\",\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ok\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn plain_grpc_service_method_routing_config(
    listen_addr: SocketAddr,
    service_addr: SocketAddr,
    method_addr: SocketAddr,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"health-service\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n        ),\n        UpstreamConfig(\n            name: \"health-check\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n            tls: Some(Insecure),\n            protocol: Http2,\n            server_name_override: Some(\"localhost\"),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Prefix(\"/\"),\n            handler: Proxy(\n                upstream: \"health-service\",\n            ),\n            grpc_service: Some(\"grpc.health.v1.Health\"),\n        ),\n        LocationConfig(\n            matcher: Prefix(\"/\"),\n            handler: Proxy(\n                upstream: \"health-check\",\n            ),\n            grpc_service: Some(\"grpc.health.v1.Health\"),\n            grpc_method: Some(\"Check\"),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("https://127.0.0.1:{}", service_addr.port()),
        format!("https://127.0.0.1:{}", method_addr.port()),
        ready_route = READY_ROUTE_CONFIG,
    )
}

fn load_certs(path: &Path) -> Vec<CertificateDer<'static>> {
    let file = File::open(path).expect("certificate file should open");
    let mut reader = BufReader::new(file);
    rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .expect("certificate PEM should parse")
}

fn load_private_key(path: &Path) -> PrivateKeyDer<'static> {
    let file = File::open(path).expect("private key file should open");
    let mut reader = BufReader::new(file);
    rustls_pemfile::private_key(&mut reader)
        .expect("private key PEM should parse")
        .expect("private key PEM should include at least one key")
}

fn temp_dir(prefix: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("{prefix}-{unique}-{id}"))
}

#[derive(Debug)]
struct InsecureServerCertVerifier {
    supported_schemes: Vec<SignatureScheme>,
}

impl InsecureServerCertVerifier {
    fn new() -> Self {
        Self {
            supported_schemes: rustls::crypto::aws_lc_rs::default_provider()
                .signature_verification_algorithms
                .supported_schemes(),
        }
    }
}

impl ServerCertVerifier for InsecureServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.supported_schemes.clone()
    }
}
