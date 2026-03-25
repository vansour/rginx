use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::future::Future;
use std::io::BufReader;
use std::path::Path;
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
use hyper_rustls::{
    ConfigBuilderExt, FixedServerNameResolver, HttpsConnector, HttpsConnectorBuilder,
};
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
use crate::metrics::Metrics;
use crate::state::SharedState;
use crate::timeout::{GrpcDeadlineBody, IdleTimeoutBody};

mod clients;
mod forward;
mod grpc_web;
mod health;
mod request_body;
mod upgrade;

const MAX_FAILOVER_ATTEMPTS: usize = 2;
const GRPC_CONTENT_TYPE_PREFIX: &str = "application/grpc";
const GRPC_WEB_CONTENT_TYPE_PREFIX: &str = "application/grpc-web";
const GRPC_WEB_TEXT_CONTENT_TYPE_PREFIX: &str = "application/grpc-web-text";
const GRPC_TIMEOUT_HEADER: &str = "grpc-timeout";
const MAX_GRPC_TIMEOUT_DIGITS: usize = 8;

pub use clients::{ProxyClient, ProxyClients};
pub use forward::{DownstreamRequestOptions, forward_request};
pub(crate) use health::PeerStatusSnapshot;
pub use health::probe_upstream_peer;

use grpc_web::GrpcWebMode;

fn build_proxy_uri(
    peer: &UpstreamPeer,
    original_uri: &Uri,
    strip_prefix: Option<&str>,
) -> Result<Uri, http::Error> {
    let original_path = original_uri.path_and_query().map(|value| value.as_str()).unwrap_or("/");

    let path_and_query = if let Some(prefix) = strip_prefix {
        if let Some(stripped) = original_path.strip_prefix(prefix) {
            if stripped.is_empty() || stripped.starts_with('?') {
                if stripped.is_empty() { "/" } else { stripped }
            } else if stripped.starts_with('/') {
                stripped
            } else {
                original_path
            }
        } else {
            original_path
        }
    } else {
        original_path
    };

    Uri::builder()
        .scheme(peer.scheme.as_str())
        .authority(peer.authority.as_str())
        .path_and_query(path_and_query)
        .build()
}

fn split_content_type(content_type: &str) -> (&str, &str) {
    let mut parts = content_type.splitn(2, ';');
    let mime = parts.next().unwrap_or_default().trim();
    let params = parts.next().unwrap_or_default().trim();
    (mime, params)
}

fn append_header_map(destination: &mut HeaderMap, source: &HeaderMap) {
    for name in source.keys() {
        for value in source.get_all(name).iter() {
            destination.append(name.clone(), value.clone());
        }
    }
}

fn sanitize_request_headers(
    headers: &mut HeaderMap,
    authority: &str,
    original_host: Option<HeaderValue>,
    client_address: &ClientAddress,
    forwarded_proto: &str,
    preserve_host: bool,
    proxy_set_headers: &[(HeaderName, HeaderValue)],
    grpc_web_mode: Option<&GrpcWebMode>,
) -> Result<(), http::header::InvalidHeaderValue> {
    let upgrade_protocol = extract_upgrade_protocol(headers);
    let te_trailers = preserved_te_trailers_value(headers);
    remove_hop_by_hop_headers(headers, upgrade_protocol.is_some());

    if preserve_host {
        if let Some(ref host) = original_host {
            headers.insert(HOST, host.clone());
        } else {
            headers.insert(HOST, HeaderValue::from_str(authority)?);
        }
    } else {
        headers.insert(HOST, HeaderValue::from_str(authority)?);
    }

    headers.insert("x-forwarded-proto", HeaderValue::from_str(forwarded_proto)?);

    if let Some(host) = original_host {
        headers.insert("x-forwarded-host", host);
    }

    headers.insert("x-forwarded-for", HeaderValue::from_str(&client_address.forwarded_for)?);

    for (name, value) in proxy_set_headers {
        headers.insert(name.clone(), value.clone());
    }

    if let Some(grpc_web_mode) = grpc_web_mode {
        headers.insert(CONTENT_TYPE, grpc_web_mode.upstream_content_type.clone());
        headers.remove(CONTENT_LENGTH);
        headers.remove("x-grpc-web");
        if headers.get(TE).is_none() {
            headers.insert(TE, HeaderValue::from_static("trailers"));
        }
    } else if headers.get(TE).is_none()
        && let Some(te_trailers) = te_trailers
    {
        headers.insert(TE, te_trailers);
    }

    if let Some(upgrade_protocol) = upgrade_protocol {
        headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(UPGRADE, upgrade_protocol);
    }

    Ok(())
}

fn preserved_te_trailers_value(headers: &HeaderMap) -> Option<HeaderValue> {
    let mut saw_te = false;

    for value in headers.get_all(TE) {
        let value = value.to_str().ok()?;

        for token in value.split(',').map(str::trim).filter(|token| !token.is_empty()) {
            saw_te = true;
            if !token.eq_ignore_ascii_case("trailers") {
                return None;
            }
        }
    }

    saw_te.then(|| HeaderValue::from_static("trailers"))
}

fn sanitize_response_headers(headers: &mut HeaderMap, preserve_upgrade: bool) {
    let upgrade_protocol = if preserve_upgrade { headers.get(UPGRADE).cloned() } else { None };
    remove_hop_by_hop_headers(headers, preserve_upgrade);

    if let Some(upgrade_protocol) = upgrade_protocol {
        headers.insert(CONNECTION, HeaderValue::from_static("upgrade"));
        headers.insert(UPGRADE, upgrade_protocol);
    }
}

fn remove_hop_by_hop_headers(headers: &mut HeaderMap, preserve_upgrade: bool) {
    let mut extra_headers = Vec::new();

    for value in headers.get_all(CONNECTION) {
        if let Ok(value) = value.to_str() {
            for item in value.split(',') {
                let trimmed = item.trim();
                if trimmed.is_empty() {
                    continue;
                }

                if let Ok(name) = HeaderName::from_bytes(trimmed.as_bytes()) {
                    extra_headers.push(name);
                }
            }
        }
    }

    for name in extra_headers {
        if preserve_upgrade && name == UPGRADE {
            continue;
        }
        headers.remove(name);
    }

    for name in [
        CONNECTION,
        PROXY_AUTHENTICATE,
        PROXY_AUTHORIZATION,
        TE,
        TRAILER,
        TRANSFER_ENCODING,
        UPGRADE,
    ] {
        if preserve_upgrade && (name == CONNECTION || name == UPGRADE) {
            continue;
        }
        headers.remove(name);
    }

    headers.remove("keep-alive");
    headers.remove("proxy-connection");
}

fn is_upgrade_request(version: Version, headers: &HeaderMap) -> bool {
    version == Version::HTTP_11 && extract_upgrade_protocol(headers).is_some()
}

fn is_upgrade_response(status: StatusCode, headers: &HeaderMap) -> bool {
    status == StatusCode::SWITCHING_PROTOCOLS && headers.contains_key(UPGRADE)
}

fn extract_upgrade_protocol(headers: &HeaderMap) -> Option<HeaderValue> {
    connection_header_contains_token(headers, "upgrade").then(|| headers.get(UPGRADE).cloned())?
}

fn connection_header_contains_token(headers: &HeaderMap, token: &str) -> bool {
    headers.get_all(CONNECTION).iter().any(|value| {
        value.to_str().ok().is_some_and(|value| {
            value.split(',').any(|item| item.trim().eq_ignore_ascii_case(token))
        })
    })
}

fn upstream_request_version(protocol: UpstreamProtocol) -> Version {
    match protocol {
        UpstreamProtocol::Http2 => Version::HTTP_2,
        UpstreamProtocol::Auto | UpstreamProtocol::Http1 => Version::HTTP_11,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::convert::Infallible;
    use std::io::{Read, Write};
    use std::net::IpAddr;
    use std::net::SocketAddr;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::thread;
    use std::time::Duration;
    use std::time::{SystemTime, UNIX_EPOCH};

    use bytes::{Bytes, BytesMut};
    use futures_util::stream;
    use http::header::{CONTENT_LENGTH, CONTENT_TYPE, HOST, TE};
    use http::{HeaderMap, HeaderValue, Method, Response, StatusCode, Uri, Version};
    use http_body_util::{BodyExt, StreamBody};
    use hyper::body::Frame;
    use rginx_core::{
        ActiveHealthCheck, Error, Upstream, UpstreamLoadBalance, UpstreamPeer, UpstreamProtocol,
        UpstreamSettings, UpstreamTls,
    };

    use super::clients::{ProxyClients, load_custom_ca_store};
    use super::forward::{
        detect_grpc_web_mode, effective_upstream_request_timeout, parse_grpc_timeout,
        wait_for_upstream_stage,
    };
    use super::grpc_web::{
        GrpcWebEncoding, GrpcWebMode, decode_grpc_web_text_chunk, decode_grpc_web_text_final,
        encode_grpc_web_text_chunk, encode_grpc_web_trailers, extract_grpc_initial_trailers,
        flush_grpc_web_text_chunk,
    };
    use super::health::{
        GrpcHealthProbeResult, GrpcHealthServingStatus, build_active_health_request,
        decode_grpc_health_check_response, encode_grpc_health_check_request,
        evaluate_grpc_health_probe_response,
    };
    use super::request_body::{
        PreparedProxyRequest, PreparedRequestBody, can_retry_peer_request, is_idempotent_method,
    };
    use super::{
        build_proxy_uri, probe_upstream_peer, sanitize_request_headers, upstream_request_version,
    };
    use crate::client_ip::{ClientAddress, ClientIpSource};
    use crate::metrics::Metrics;

    #[test]
    fn proxy_uri_keeps_path_and_query() {
        let peer = UpstreamPeer {
            url: "http://127.0.0.1:9000".to_string(),
            scheme: "http".to_string(),
            authority: "127.0.0.1:9000".to_string(),
            weight: 1,
            backup: false,
        };

        let uri = build_proxy_uri(&peer, &"/api/demo?x=1".parse().unwrap(), None).unwrap();
        assert_eq!(uri, "http://127.0.0.1:9000/api/demo?x=1".parse::<http::Uri>().unwrap());
    }

    #[test]
    fn proxy_uri_keeps_https_scheme() {
        let peer = UpstreamPeer {
            url: "https://example.com".to_string(),
            scheme: "https".to_string(),
            authority: "example.com".to_string(),
            weight: 1,
            backup: false,
        };

        let uri = build_proxy_uri(&peer, &"/healthz".parse().unwrap(), None).unwrap();
        assert_eq!(uri, "https://example.com/healthz".parse::<http::Uri>().unwrap());
    }

    #[test]
    fn sanitize_request_headers_overwrites_x_forwarded_for_with_sanitized_chain() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("client.example"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("spoofed"));

        let client_address = ClientAddress {
            peer_addr: "10.2.3.4:4000".parse().unwrap(),
            client_ip: "198.51.100.9".parse().unwrap(),
            forwarded_for: "198.51.100.9, 10.1.2.3, 10.2.3.4".to_string(),
            source: ClientIpSource::XForwardedFor,
        };

        sanitize_request_headers(
            &mut headers,
            "127.0.0.1:9000",
            Some(HeaderValue::from_static("client.example")),
            &client_address,
            "https",
            false,
            &[],
            None,
        )
        .expect("header sanitization should succeed");

        assert_eq!(headers.get(HOST).unwrap(), "127.0.0.1:9000");
        assert_eq!(headers.get("x-forwarded-host").unwrap(), "client.example");
        assert_eq!(headers.get("x-forwarded-for").unwrap(), "198.51.100.9, 10.1.2.3, 10.2.3.4");
        assert_eq!(headers.get("x-forwarded-proto").unwrap(), "https");
    }

    #[test]
    fn sanitize_request_headers_preserves_upgrade_handshake() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("client.example"));
        headers.insert(http::header::CONNECTION, HeaderValue::from_static("keep-alive, Upgrade"));
        headers.insert(http::header::UPGRADE, HeaderValue::from_static("websocket"));

        let client_address = ClientAddress {
            peer_addr: "10.2.3.4:4000".parse().unwrap(),
            client_ip: "198.51.100.9".parse().unwrap(),
            forwarded_for: "198.51.100.9".to_string(),
            source: ClientIpSource::SocketPeer,
        };

        sanitize_request_headers(
            &mut headers,
            "127.0.0.1:9000",
            Some(HeaderValue::from_static("client.example")),
            &client_address,
            "http",
            false,
            &[],
            None,
        )
        .expect("header sanitization should succeed");

        assert_eq!(headers.get(HOST).unwrap(), "127.0.0.1:9000");
        assert_eq!(headers.get(http::header::CONNECTION).unwrap(), "upgrade");
        assert_eq!(headers.get(http::header::UPGRADE).unwrap(), "websocket");
    }

    #[test]
    fn sanitize_request_headers_preserves_te_trailers() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("client.example"));
        headers.insert(http::header::TE, HeaderValue::from_static("trailers"));

        let client_address = ClientAddress {
            peer_addr: "10.2.3.4:4000".parse().unwrap(),
            client_ip: "198.51.100.9".parse().unwrap(),
            forwarded_for: "198.51.100.9".to_string(),
            source: ClientIpSource::SocketPeer,
        };

        sanitize_request_headers(
            &mut headers,
            "127.0.0.1:9000",
            Some(HeaderValue::from_static("client.example")),
            &client_address,
            "https",
            false,
            &[],
            None,
        )
        .expect("header sanitization should succeed");

        assert_eq!(headers.get(HOST).unwrap(), "127.0.0.1:9000");
        assert_eq!(headers.get(http::header::TE).unwrap(), "trailers");
    }

    #[test]
    fn sanitize_request_headers_drops_non_trailers_te_tokens() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("client.example"));
        headers.insert(http::header::TE, HeaderValue::from_static("trailers, gzip"));

        let client_address = ClientAddress {
            peer_addr: "10.2.3.4:4000".parse().unwrap(),
            client_ip: "198.51.100.9".parse().unwrap(),
            forwarded_for: "198.51.100.9".to_string(),
            source: ClientIpSource::SocketPeer,
        };

        sanitize_request_headers(
            &mut headers,
            "127.0.0.1:9000",
            Some(HeaderValue::from_static("client.example")),
            &client_address,
            "https",
            false,
            &[],
            None,
        )
        .expect("header sanitization should succeed");

        assert_eq!(headers.get(HOST).unwrap(), "127.0.0.1:9000");
        assert!(headers.get(http::header::TE).is_none());
    }

    #[test]
    fn detect_grpc_web_mode_rewrites_binary_content_type() {
        let mut headers = HeaderMap::new();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/grpc-web+proto; charset=utf-8"),
        );

        let mode = detect_grpc_web_mode(&headers)
            .expect("binary grpc-web should be supported")
            .expect("grpc-web content-type should be detected");

        assert_eq!(mode.downstream_content_type, "application/grpc-web+proto; charset=utf-8");
        assert_eq!(mode.upstream_content_type, "application/grpc+proto; charset=utf-8");
        assert_eq!(mode.encoding, GrpcWebEncoding::Binary);
    }

    #[test]
    fn detect_grpc_web_mode_rewrites_text_content_type() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc-web-text+proto"));

        let mode = detect_grpc_web_mode(&headers)
            .expect("grpc-web-text should be supported")
            .expect("grpc-web-text content-type should be detected");

        assert_eq!(mode.downstream_content_type, "application/grpc-web-text+proto");
        assert_eq!(mode.upstream_content_type, "application/grpc+proto");
        assert_eq!(mode.encoding, GrpcWebEncoding::Text);
    }

    #[test]
    fn parse_grpc_timeout_accepts_supported_units() {
        let cases = [
            ("1H", Duration::from_secs(60 * 60)),
            ("2M", Duration::from_secs(2 * 60)),
            ("3S", Duration::from_secs(3)),
            ("4m", Duration::from_millis(4)),
            ("5u", Duration::from_micros(5)),
            ("6n", Duration::from_nanos(6)),
            ("0n", Duration::ZERO),
        ];

        for (value, expected) in cases {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc"));
            headers.insert("grpc-timeout", HeaderValue::from_str(value).unwrap());

            let timeout = parse_grpc_timeout(&headers)
                .expect("grpc-timeout should parse")
                .expect("grpc-timeout should be present");

            assert_eq!(timeout, expected, "grpc-timeout {value} should parse correctly");
        }
    }

    #[test]
    fn parse_grpc_timeout_rejects_invalid_values() {
        for value in ["", "1", "abc", "123456789m", "1x", "1 m"] {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc"));
            headers.insert("grpc-timeout", HeaderValue::from_str(value).unwrap());

            let error = parse_grpc_timeout(&headers).expect_err("invalid grpc-timeout should fail");
            assert!(error.contains("invalid grpc-timeout header"));
        }
    }

    #[test]
    fn effective_upstream_request_timeout_prefers_shorter_grpc_deadline() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc"));
        headers.insert("grpc-timeout", HeaderValue::from_static("250m"));

        let timeout = effective_upstream_request_timeout(&headers, Duration::from_secs(30))
            .expect("grpc timeout should compute");

        assert_eq!(timeout, Duration::from_millis(250));
    }

    #[test]
    fn effective_upstream_request_timeout_ignores_non_grpc_requests() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        headers.insert("grpc-timeout", HeaderValue::from_static("broken"));

        let timeout = effective_upstream_request_timeout(&headers, Duration::from_secs(30))
            .expect("non-gRPC requests should ignore grpc-timeout");

        assert_eq!(timeout, Duration::from_secs(30));
    }

    #[test]
    fn sanitize_request_headers_translates_grpc_web_requests() {
        let mut headers = HeaderMap::new();
        headers.insert(HOST, HeaderValue::from_static("client.example"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc-web+proto"));
        headers.insert("x-grpc-web", HeaderValue::from_static("1"));

        let client_address = ClientAddress {
            peer_addr: "10.2.3.4:4000".parse().unwrap(),
            client_ip: "198.51.100.9".parse().unwrap(),
            forwarded_for: "198.51.100.9".to_string(),
            source: ClientIpSource::SocketPeer,
        };
        let grpc_web_mode = GrpcWebMode {
            downstream_content_type: HeaderValue::from_static("application/grpc-web+proto"),
            upstream_content_type: HeaderValue::from_static("application/grpc+proto"),
            encoding: GrpcWebEncoding::Binary,
        };

        sanitize_request_headers(
            &mut headers,
            "127.0.0.1:9000",
            Some(HeaderValue::from_static("client.example")),
            &client_address,
            "http",
            false,
            &[],
            Some(&grpc_web_mode),
        )
        .expect("header sanitization should succeed");

        assert_eq!(headers.get(HOST).unwrap(), "127.0.0.1:9000");
        assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "application/grpc+proto");
        assert_eq!(headers.get(http::header::TE).unwrap(), "trailers");
        assert!(headers.get("x-grpc-web").is_none());
    }

    #[test]
    fn grpc_web_text_helpers_round_trip_streamed_payloads() {
        let mut encoded = BytesMut::new();
        let first =
            encode_grpc_web_text_chunk(&mut encoded, b"hello").expect("first chunk should encode");
        let second = encode_grpc_web_text_chunk(&mut encoded, b" world")
            .expect("second chunk should encode");
        let tail = flush_grpc_web_text_chunk(&mut encoded).expect("tail should flush");

        let mut decoder = BytesMut::new();
        let decoded_first = decode_grpc_web_text_chunk(&mut decoder, &first)
            .expect("first chunk should decode")
            .expect("first chunk should yield bytes");
        let decoded_second = decode_grpc_web_text_chunk(&mut decoder, &second)
            .expect("second chunk should decode")
            .expect("second chunk should yield bytes");
        let decoded_tail = decode_grpc_web_text_chunk(&mut decoder, &tail)
            .expect("tail chunk should decode")
            .expect("tail chunk should yield bytes");
        let final_flush =
            decode_grpc_web_text_final(&mut decoder).expect("final flush should decode cleanly");

        assert_eq!(decoded_first, Bytes::from_static(b"hel"));
        assert_eq!(decoded_second, Bytes::from_static(b"lo wor"));
        assert_eq!(decoded_tail, Bytes::from_static(b"ld"));
        assert!(final_flush.is_none());
    }

    #[test]
    fn encode_grpc_web_trailers_uses_http1_header_block() {
        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", HeaderValue::from_static("0"));
        trailers.insert("grpc-message", HeaderValue::from_static("ok"));

        let encoded = encode_grpc_web_trailers(&trailers);
        assert_eq!(encoded[0], 0x80);
        let len = u32::from_be_bytes([encoded[1], encoded[2], encoded[3], encoded[4]]) as usize;
        assert_eq!(len, encoded.len() - 5);

        let block = std::str::from_utf8(&encoded[5..]).expect("trailer block should be utf-8");
        assert!(block.contains("grpc-status: 0\r\n"));
        assert!(block.contains("grpc-message: ok\r\n"));
    }

    #[test]
    fn extract_grpc_initial_trailers_removes_grpc_status_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("grpc-status", HeaderValue::from_static("7"));
        headers.insert("grpc-message", HeaderValue::from_static("denied"));
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/grpc"));

        let trailers = extract_grpc_initial_trailers(&mut headers)
            .expect("grpc status headers should extract");

        assert_eq!(trailers.get("grpc-status").unwrap(), "7");
        assert_eq!(trailers.get("grpc-message").unwrap(), "denied");
        assert!(headers.get("grpc-status").is_none());
        assert!(headers.get("grpc-message").is_none());
        assert_eq!(headers.get(CONTENT_TYPE).unwrap(), "application/grpc");
    }

    #[tokio::test]
    async fn build_active_health_request_builds_grpc_probe_request() {
        let upstream = Upstream::new(
            "grpc-backend".to_string(),
            vec![peer("https://example.com")],
            UpstreamTls::NativeRoots,
            upstream_settings(UpstreamProtocol::Auto),
        );
        let peer = upstream.peers[0].clone();
        let check = ActiveHealthCheck {
            path: "/grpc.health.v1.Health/Check".to_string(),
            grpc_service: Some("grpc.health.v1.Health".to_string()),
            interval: Duration::from_secs(5),
            timeout: Duration::from_secs(1),
            healthy_successes_required: 1,
        };

        let request =
            build_active_health_request(&upstream, &peer, &check).expect("request should build");

        assert_eq!(request.method(), Method::POST);
        assert_eq!(request.version(), Version::HTTP_2);
        assert_eq!(
            request.uri(),
            &"https://example.com/grpc.health.v1.Health/Check".parse::<Uri>().unwrap()
        );
        assert_eq!(request.headers().get(HOST).unwrap(), "example.com");
        assert_eq!(request.headers().get(CONTENT_TYPE).unwrap(), "application/grpc");
        assert_eq!(request.headers().get(TE).unwrap(), "trailers");
        let content_length = request
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
            .expect("content-length should be present");

        let body =
            request.into_body().collect().await.expect("request body should collect").to_bytes();
        assert_eq!(content_length, body.len().to_string());
        assert_eq!(body, encode_grpc_health_check_request("grpc.health.v1.Health"));
    }

    #[test]
    fn decode_grpc_health_check_response_reads_serving_status() {
        let encoded = grpc_health_response_body(1);

        let serving_status =
            decode_grpc_health_check_response(&encoded).expect("response should decode");

        assert_eq!(serving_status, Some(GrpcHealthServingStatus::Serving));
    }

    #[tokio::test]
    async fn evaluate_grpc_health_probe_response_recognizes_serving_response() {
        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", HeaderValue::from_static("0"));
        let body = StreamBody::new(stream::iter(vec![
            Ok::<_, Infallible>(Frame::data(grpc_health_response_body(1))),
            Ok(Frame::trailers(trailers)),
        ]));
        let response =
            Response::builder().status(StatusCode::OK).body(body).expect("response should build");

        let result =
            evaluate_grpc_health_probe_response(response).await.expect("response should evaluate");

        assert!(matches!(result, GrpcHealthProbeResult::Serving));
    }

    #[test]
    fn upstream_request_version_follows_upstream_protocol() {
        assert_eq!(upstream_request_version(UpstreamProtocol::Auto), Version::HTTP_11);
        assert_eq!(upstream_request_version(UpstreamProtocol::Http1), Version::HTTP_11);
        assert_eq!(upstream_request_version(UpstreamProtocol::Http2), Version::HTTP_2);
    }

    #[test]
    fn load_custom_ca_store_accepts_pem_files() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rginx-custom-ca-{unique}.pem"));
        std::fs::write(&path, TEST_CA_CERT_PEM).expect("PEM file should be written");

        let store = load_custom_ca_store(&path).expect("PEM CA should load");
        assert!(!store.is_empty());

        std::fs::remove_file(path).expect("temp PEM file should be removed");
    }

    #[test]
    fn proxy_clients_can_select_insecure_and_custom_ca_modes() {
        let insecure = Upstream::new(
            "insecure".to_string(),
            vec![UpstreamPeer {
                url: "https://localhost:9443".to_string(),
                scheme: "https".to_string(),
                authority: "localhost:9443".to_string(),
                weight: 1,
                backup: false,
            }],
            UpstreamTls::Insecure,
            upstream_settings(UpstreamProtocol::Auto),
        );

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rginx-custom-ca-select-{unique}.pem"));
        std::fs::write(&path, TEST_CA_CERT_PEM).expect("PEM file should be written");

        let custom = Upstream::new(
            "custom".to_string(),
            vec![UpstreamPeer {
                url: "https://localhost:9443".to_string(),
                scheme: "https".to_string(),
                authority: "localhost:9443".to_string(),
                weight: 1,
                backup: false,
            }],
            UpstreamTls::CustomCa { ca_cert_path: path.clone() },
            upstream_settings(UpstreamProtocol::Auto),
        );

        let snapshot = rginx_core::ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings {
                shutdown_timeout: std::time::Duration::from_secs(1),
                worker_threads: None,
                accept_workers: 1,
            },
            server: rginx_core::Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                request_body_read_timeout: None,
                response_write_timeout: None,
                access_log_format: None,
                tls: None,
            },
            default_vhost: rginx_core::VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            upstreams: HashMap::from([
                ("insecure".to_string(), Arc::new(insecure)),
                ("custom".to_string(), Arc::new(custom)),
            ]),
        };

        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
        assert!(clients.for_upstream(snapshot.upstreams["insecure"].as_ref()).is_ok());
        assert!(clients.for_upstream(snapshot.upstreams["custom"].as_ref()).is_ok());

        std::fs::remove_file(path).expect("temp PEM file should be removed");
    }

    #[test]
    fn proxy_clients_cache_distinguishes_server_name_override() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("rginx-custom-ca-override-{unique}.pem"));
        std::fs::write(&path, TEST_CA_CERT_PEM).expect("PEM file should be written");

        let peer = UpstreamPeer {
            url: "https://127.0.0.1:9443".to_string(),
            scheme: "https".to_string(),
            authority: "127.0.0.1:9443".to_string(),
            weight: 1,
            backup: false,
        };
        let first = Upstream::new(
            "first".to_string(),
            vec![peer.clone()],
            UpstreamTls::CustomCa { ca_cert_path: path.clone() },
            UpstreamSettings {
                server_name_override: Some("api-a.internal".to_string()),
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        );
        let second = Upstream::new(
            "second".to_string(),
            vec![peer.clone()],
            UpstreamTls::CustomCa { ca_cert_path: path.clone() },
            UpstreamSettings {
                server_name_override: Some("api-b.internal".to_string()),
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        );
        let duplicate = Upstream::new(
            "duplicate".to_string(),
            vec![peer],
            UpstreamTls::CustomCa { ca_cert_path: path.clone() },
            UpstreamSettings {
                server_name_override: Some("api-a.internal".to_string()),
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        );

        let snapshot = rginx_core::ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings {
                shutdown_timeout: std::time::Duration::from_secs(1),
                worker_threads: None,
                accept_workers: 1,
            },
            server: rginx_core::Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                request_body_read_timeout: None,
                response_write_timeout: None,
                access_log_format: None,
                tls: None,
            },
            default_vhost: rginx_core::VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            upstreams: HashMap::from([
                ("first".to_string(), Arc::new(first)),
                ("second".to_string(), Arc::new(second)),
                ("duplicate".to_string(), Arc::new(duplicate)),
            ]),
        };

        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
        assert_eq!(clients.cached_client_count(), 2);
        assert!(clients.for_upstream(snapshot.upstreams["first"].as_ref()).is_ok());
        assert!(clients.for_upstream(snapshot.upstreams["second"].as_ref()).is_ok());
        assert!(clients.for_upstream(snapshot.upstreams["duplicate"].as_ref()).is_ok());

        std::fs::remove_file(path).expect("temp PEM file should be removed");
    }

    #[test]
    fn proxy_clients_cache_distinguishes_upstream_protocol() {
        let peer = UpstreamPeer {
            url: "https://127.0.0.1:9443".to_string(),
            scheme: "https".to_string(),
            authority: "127.0.0.1:9443".to_string(),
            weight: 1,
            backup: false,
        };
        let auto = Upstream::new(
            "auto".to_string(),
            vec![peer.clone()],
            UpstreamTls::NativeRoots,
            upstream_settings(UpstreamProtocol::Auto),
        );
        let http1 = Upstream::new(
            "http1".to_string(),
            vec![peer.clone()],
            UpstreamTls::NativeRoots,
            upstream_settings(UpstreamProtocol::Http1),
        );
        let http2 = Upstream::new(
            "http2".to_string(),
            vec![peer],
            UpstreamTls::NativeRoots,
            upstream_settings(UpstreamProtocol::Http2),
        );

        let snapshot = rginx_core::ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings {
                shutdown_timeout: Duration::from_secs(1),
                worker_threads: None,
                accept_workers: 1,
            },
            server: rginx_core::Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                request_body_read_timeout: None,
                response_write_timeout: None,
                access_log_format: None,
                tls: None,
            },
            default_vhost: rginx_core::VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            upstreams: HashMap::from([
                ("auto".to_string(), Arc::new(auto)),
                ("http1".to_string(), Arc::new(http1)),
                ("http2".to_string(), Arc::new(http2)),
            ]),
        };

        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
        assert_eq!(clients.cached_client_count(), 3);
    }

    #[tokio::test]
    async fn wait_for_upstream_stage_times_out() {
        let timeout = Duration::from_millis(25);

        let error = wait_for_upstream_stage(timeout, "backend", "request", async {
            tokio::time::sleep(Duration::from_millis(100)).await;
        })
        .await
        .expect_err("slow future should time out");

        assert!(matches!(error, Error::Server(message) if message.contains("timed out")));
    }

    #[test]
    fn upstream_next_peers_returns_distinct_failover_candidates() {
        let upstream = Upstream::new(
            "backend".to_string(),
            vec![
                peer("http://127.0.0.1:9000"),
                peer("http://127.0.0.1:9001"),
                peer("http://127.0.0.1:9002"),
            ],
            UpstreamTls::NativeRoots,
            upstream_settings(UpstreamProtocol::Auto),
        );

        let first = upstream.next_peers(2);
        let second = upstream.next_peers(2);

        assert_eq!(
            first.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9000", "http://127.0.0.1:9001",]
        );
        assert_eq!(
            second.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9001", "http://127.0.0.1:9002",]
        );
    }

    #[test]
    fn replayable_idempotent_requests_retry_once() {
        let prepared = PreparedProxyRequest {
            method: Method::GET,
            uri: Uri::from_static("/"),
            headers: HeaderMap::new(),
            body: PreparedRequestBody::Replayable { body: Bytes::new(), trailers: None },
        };
        let peers = vec![peer("http://127.0.0.1:9000"), peer("http://127.0.0.1:9001")];

        assert!(can_retry_peer_request(&prepared, &peers, 0));
        assert!(!can_retry_peer_request(&prepared, &peers, 1));
    }

    #[test]
    fn streaming_requests_do_not_retry() {
        let prepared = PreparedProxyRequest {
            method: Method::GET,
            uri: Uri::from_static("/"),
            headers: HeaderMap::new(),
            body: PreparedRequestBody::Streaming(None),
        };
        let peers = vec![peer("http://127.0.0.1:9000"), peer("http://127.0.0.1:9001")];

        assert!(!can_retry_peer_request(&prepared, &peers, 0));
    }

    #[test]
    fn idempotent_method_detection_matches_retry_policy() {
        assert!(is_idempotent_method(&Method::GET));
        assert!(is_idempotent_method(&Method::PUT));
        assert!(is_idempotent_method(&Method::DELETE));
        assert!(!is_idempotent_method(&Method::POST));
        assert!(!is_idempotent_method(&Method::PATCH));
    }

    #[test]
    fn ip_hash_keeps_the_same_primary_peer_for_the_same_client_ip() {
        let upstream = Upstream::new(
            "backend".to_string(),
            vec![
                peer("http://127.0.0.1:9000"),
                peer("http://127.0.0.1:9001"),
                peer("http://127.0.0.1:9002"),
            ],
            UpstreamTls::NativeRoots,
            UpstreamSettings {
                load_balance: UpstreamLoadBalance::IpHash,
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        );
        let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let first = clients.select_peers(
            snapshot.upstreams["backend"].as_ref(),
            client_ip("198.51.100.10"),
            2,
        );
        let second = clients.select_peers(
            snapshot.upstreams["backend"].as_ref(),
            client_ip("198.51.100.10"),
            2,
        );

        assert_eq!(
            first.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            second.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn ip_hash_skips_unhealthy_primary_and_uses_the_next_peer() {
        let upstream = Upstream::new(
            "backend".to_string(),
            vec![
                peer("http://127.0.0.1:9000"),
                peer("http://127.0.0.1:9001"),
                peer("http://127.0.0.1:9002"),
            ],
            UpstreamTls::NativeRoots,
            UpstreamSettings {
                load_balance: UpstreamLoadBalance::IpHash,
                unhealthy_after_failures: 1,
                unhealthy_cooldown: Duration::from_secs(30),
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        );
        let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let initial = clients.select_peers(
            snapshot.upstreams["backend"].as_ref(),
            client_ip("198.51.100.10"),
            2,
        );
        let primary = initial.peers[0].url.clone();
        let fallback = initial.peers[1].url.clone();

        let failure = clients.record_peer_failure("backend", &primary);
        assert!(failure.entered_cooldown);

        let selected = clients.select_peers(
            snapshot.upstreams["backend"].as_ref(),
            client_ip("198.51.100.10"),
            2,
        );
        assert_eq!(selected.skipped_unhealthy, 1);
        assert_eq!(selected.peers[0].url, fallback);
    }

    #[test]
    fn ip_hash_distributes_multiple_client_ips_across_peers() {
        let upstream = Upstream::new(
            "backend".to_string(),
            vec![
                peer("http://127.0.0.1:9000"),
                peer("http://127.0.0.1:9001"),
                peer("http://127.0.0.1:9002"),
            ],
            UpstreamTls::NativeRoots,
            UpstreamSettings {
                load_balance: UpstreamLoadBalance::IpHash,
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        );
        let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let unique_primaries = (1..=16)
            .map(|suffix| {
                let ip = format!("198.51.100.{suffix}");
                clients
                    .select_peers(snapshot.upstreams["backend"].as_ref(), client_ip(&ip), 1)
                    .peers[0]
                    .url
                    .clone()
            })
            .collect::<std::collections::HashSet<_>>();

        assert!(
            unique_primaries.len() >= 2,
            "expected ip_hash to spread clients across at least two peers"
        );
    }

    #[test]
    fn weighted_ip_hash_prefers_heavier_peers() {
        let upstream = Upstream::new(
            "backend".to_string(),
            vec![
                peer_with_weight("http://127.0.0.1:9000", 5),
                peer_with_weight("http://127.0.0.1:9001", 1),
            ],
            UpstreamTls::NativeRoots,
            UpstreamSettings {
                load_balance: UpstreamLoadBalance::IpHash,
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        );
        let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let heavy = (0..=255)
            .filter(|suffix| {
                let ip = format!("198.51.100.{suffix}");
                clients
                    .select_peers(snapshot.upstreams["backend"].as_ref(), client_ip(&ip), 1)
                    .peers[0]
                    .url
                    == "http://127.0.0.1:9000"
            })
            .count();

        assert!(heavy > 128, "expected weighted ip_hash to prefer the heavier peer");
    }

    #[test]
    fn backup_peer_is_only_used_as_retry_candidate_while_primary_is_healthy() {
        let upstream = Upstream::new(
            "backend".to_string(),
            vec![peer("http://127.0.0.1:9000"), peer_with_role("http://127.0.0.1:9010", 1, true)],
            UpstreamTls::NativeRoots,
            upstream_settings(UpstreamProtocol::Auto),
        );
        let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let primary_only = clients.select_peers(
            snapshot.upstreams["backend"].as_ref(),
            client_ip("198.51.100.10"),
            1,
        );
        assert_eq!(
            primary_only.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9000"]
        );

        let with_retry = clients.select_peers(
            snapshot.upstreams["backend"].as_ref(),
            client_ip("198.51.100.10"),
            2,
        );
        assert_eq!(
            with_retry.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9000", "http://127.0.0.1:9010"]
        );
    }

    #[test]
    fn backup_peer_is_selected_when_primary_pool_is_unhealthy() {
        let upstream = Upstream::new(
            "backend".to_string(),
            vec![peer("http://127.0.0.1:9000"), peer_with_role("http://127.0.0.1:9010", 1, true)],
            UpstreamTls::NativeRoots,
            UpstreamSettings {
                unhealthy_after_failures: 1,
                unhealthy_cooldown: Duration::from_secs(30),
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        );
        let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
        assert!(failure.entered_cooldown);

        let selected = clients.select_peers(
            snapshot.upstreams["backend"].as_ref(),
            client_ip("198.51.100.10"),
            1,
        );
        assert_eq!(selected.skipped_unhealthy, 1);
        assert_eq!(
            selected.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9010"]
        );
    }

    #[test]
    fn least_conn_prefers_peers_with_fewer_active_requests() {
        let upstream = Upstream::new(
            "backend".to_string(),
            vec![
                peer("http://127.0.0.1:9000"),
                peer("http://127.0.0.1:9001"),
                peer("http://127.0.0.1:9002"),
            ],
            UpstreamTls::NativeRoots,
            UpstreamSettings {
                load_balance: UpstreamLoadBalance::LeastConn,
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        );
        let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let _peer_a_1 = clients.track_active_request("backend", "http://127.0.0.1:9000");
        let _peer_a_2 = clients.track_active_request("backend", "http://127.0.0.1:9000");
        let _peer_b_1 = clients.track_active_request("backend", "http://127.0.0.1:9001");

        let selected = clients.select_peers(
            snapshot.upstreams["backend"].as_ref(),
            client_ip("198.51.100.10"),
            3,
        );

        assert_eq!(
            selected.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9002", "http://127.0.0.1:9001", "http://127.0.0.1:9000",]
        );
    }

    #[test]
    fn least_conn_uses_configured_peer_order_to_break_ties() {
        let upstream = Upstream::new(
            "backend".to_string(),
            vec![
                peer("http://127.0.0.1:9000"),
                peer("http://127.0.0.1:9001"),
                peer("http://127.0.0.1:9002"),
            ],
            UpstreamTls::NativeRoots,
            UpstreamSettings {
                load_balance: UpstreamLoadBalance::LeastConn,
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        );
        let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let selected = clients.select_peers(
            snapshot.upstreams["backend"].as_ref(),
            client_ip("198.51.100.10"),
            3,
        );

        assert_eq!(
            selected.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9000", "http://127.0.0.1:9001", "http://127.0.0.1:9002",]
        );
    }

    #[test]
    fn weighted_least_conn_prefers_higher_capacity_peer_when_projected_load_ties() {
        let upstream = Upstream::new(
            "backend".to_string(),
            vec![
                peer_with_weight("http://127.0.0.1:9000", 3),
                peer_with_weight("http://127.0.0.1:9001", 1),
            ],
            UpstreamTls::NativeRoots,
            UpstreamSettings {
                load_balance: UpstreamLoadBalance::LeastConn,
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        );
        let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let _peer_a_1 = clients.track_active_request("backend", "http://127.0.0.1:9000");
        let _peer_a_2 = clients.track_active_request("backend", "http://127.0.0.1:9000");

        let selected = clients.select_peers(
            snapshot.upstreams["backend"].as_ref(),
            client_ip("198.51.100.10"),
            2,
        );

        assert_eq!(
            selected.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9000", "http://127.0.0.1:9001"]
        );
    }

    #[test]
    fn least_conn_ignores_backup_peers_while_primary_pool_is_available() {
        let upstream = Upstream::new(
            "backend".to_string(),
            vec![peer("http://127.0.0.1:9000"), peer_with_role("http://127.0.0.1:9010", 1, true)],
            UpstreamTls::NativeRoots,
            UpstreamSettings {
                load_balance: UpstreamLoadBalance::LeastConn,
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        );
        let snapshot = snapshot_with_upstream("backend", Arc::new(upstream));
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let selected = clients.select_peers(
            snapshot.upstreams["backend"].as_ref(),
            client_ip("198.51.100.10"),
            1,
        );
        assert_eq!(
            selected.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9000"]
        );
    }

    #[test]
    fn unhealthy_peer_is_skipped_after_consecutive_failures() {
        let snapshot = snapshot_with_upstream_policy(
            "backend",
            vec![peer("http://127.0.0.1:9000"), peer("http://127.0.0.1:9001")],
            2,
            Duration::from_secs(30),
        );
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let first = clients.select_peers(
            snapshot.upstreams["backend"].as_ref(),
            client_ip("198.51.100.10"),
            2,
        );
        assert_eq!(first.skipped_unhealthy, 0);
        assert_eq!(
            first.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9000", "http://127.0.0.1:9001"]
        );

        let first_failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
        assert_eq!(first_failure.consecutive_failures, 1);
        assert!(!first_failure.entered_cooldown);

        let second_failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
        assert_eq!(second_failure.consecutive_failures, 2);
        assert!(second_failure.entered_cooldown);

        let selected = clients.select_peers(
            snapshot.upstreams["backend"].as_ref(),
            client_ip("198.51.100.10"),
            2,
        );
        assert_eq!(selected.skipped_unhealthy, 1);
        assert_eq!(
            selected.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9001"]
        );
    }

    #[tokio::test]
    async fn unhealthy_peer_recovers_after_cooldown() {
        let snapshot = snapshot_with_upstream_policy(
            "backend",
            vec![peer("http://127.0.0.1:9000"), peer("http://127.0.0.1:9001")],
            1,
            Duration::from_millis(20),
        );
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
        assert!(failure.entered_cooldown);

        let immediately = clients.select_peers(
            snapshot.upstreams["backend"].as_ref(),
            client_ip("198.51.100.10"),
            2,
        );
        assert_eq!(immediately.skipped_unhealthy, 1);
        assert_eq!(
            immediately.peers.iter().map(|peer| peer.url.as_str()).collect::<Vec<_>>(),
            vec!["http://127.0.0.1:9001"]
        );

        tokio::time::sleep(Duration::from_millis(30)).await;

        let recovered = clients.select_peers(
            snapshot.upstreams["backend"].as_ref(),
            client_ip("198.51.100.10"),
            2,
        );
        assert_eq!(recovered.skipped_unhealthy, 0);
        assert_eq!(recovered.peers.len(), 2);
    }

    #[test]
    fn successful_request_resets_peer_failure_count() {
        let snapshot = snapshot_with_upstream_policy(
            "backend",
            vec![peer("http://127.0.0.1:9000")],
            2,
            Duration::from_secs(30),
        );
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let failure = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
        assert_eq!(failure.consecutive_failures, 1);

        clients.record_peer_success("backend", "http://127.0.0.1:9000");

        let after_reset = clients.record_peer_failure("backend", "http://127.0.0.1:9000");
        assert_eq!(after_reset.consecutive_failures, 1);
        assert!(!after_reset.entered_cooldown);
    }

    #[test]
    fn peer_health_policy_is_applied_per_upstream() {
        let fast_fail = Arc::new(Upstream::new(
            "fast-fail".to_string(),
            vec![peer("http://127.0.0.1:9000")],
            UpstreamTls::NativeRoots,
            UpstreamSettings {
                unhealthy_after_failures: 1,
                unhealthy_cooldown: Duration::from_secs(30),
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        ));
        let tolerant = Arc::new(Upstream::new(
            "tolerant".to_string(),
            vec![peer("http://127.0.0.1:9010")],
            UpstreamTls::NativeRoots,
            UpstreamSettings {
                unhealthy_after_failures: 3,
                unhealthy_cooldown: Duration::from_secs(30),
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        ));

        let snapshot = rginx_core::ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings {
                shutdown_timeout: Duration::from_secs(1),
                worker_threads: None,
                accept_workers: 1,
            },
            server: rginx_core::Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                request_body_read_timeout: None,
                response_write_timeout: None,
                access_log_format: None,
                tls: None,
            },
            default_vhost: rginx_core::VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            upstreams: HashMap::from([
                ("fast-fail".to_string(), fast_fail.clone()),
                ("tolerant".to_string(), tolerant.clone()),
            ]),
        };
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        let fast_failure = clients.record_peer_failure("fast-fail", "http://127.0.0.1:9000");
        assert!(fast_failure.entered_cooldown);

        let tolerant_failure = clients.record_peer_failure("tolerant", "http://127.0.0.1:9010");
        assert_eq!(tolerant_failure.consecutive_failures, 1);
        assert!(!tolerant_failure.entered_cooldown);

        let fast_selected = clients.select_peers(
            snapshot.upstreams["fast-fail"].as_ref(),
            client_ip("198.51.100.10"),
            1,
        );
        assert!(fast_selected.peers.is_empty());
        assert_eq!(fast_selected.skipped_unhealthy, 1);

        let tolerant_selected = clients.select_peers(
            snapshot.upstreams["tolerant"].as_ref(),
            client_ip("198.51.100.10"),
            1,
        );
        assert_eq!(tolerant_selected.peers.len(), 1);
        assert_eq!(tolerant_selected.skipped_unhealthy, 0);
    }

    #[test]
    fn active_health_requires_recovery_threshold_before_peer_is_reused() {
        let snapshot = snapshot_with_active_health(
            "backend",
            vec![peer("http://127.0.0.1:9000")],
            "/healthz",
            2,
        );
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");

        assert_eq!(
            clients
                .select_peers(snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 1)
                .peers
                .len(),
            1
        );
        assert!(clients.record_active_peer_failure("backend", "http://127.0.0.1:9000"));
        assert!(
            clients
                .select_peers(snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 1)
                .peers
                .is_empty()
        );

        let first_success =
            clients.record_active_peer_success("backend", "http://127.0.0.1:9000", 2);
        assert!(!first_success.recovered);
        assert_eq!(first_success.consecutive_successes, 1);
        assert!(
            clients
                .select_peers(snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 1)
                .peers
                .is_empty()
        );

        let second_success =
            clients.record_active_peer_success("backend", "http://127.0.0.1:9000", 2);
        assert!(second_success.recovered);
        assert_eq!(second_success.consecutive_successes, 2);
        assert_eq!(
            clients
                .select_peers(snapshot.upstreams["backend"].as_ref(), client_ip("198.51.100.10"), 1)
                .peers
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn active_health_probe_tracks_status_transitions() {
        let statuses = Arc::new(Mutex::new(VecDeque::from([
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::OK,
            StatusCode::OK,
        ])));
        let listen_addr = spawn_status_server(statuses).await;
        let peer_url = format!("http://{listen_addr}");
        let snapshot = snapshot_with_active_health("backend", vec![peer(&peer_url)], "/healthz", 2);
        let clients = ProxyClients::from_config(&snapshot).expect("clients should build");
        let metrics = Metrics::default();
        let upstream = snapshot.upstreams["backend"].clone();
        let peer = upstream.peers[0].clone();

        probe_upstream_peer(clients.clone(), metrics.clone(), upstream.clone(), peer.clone()).await;
        assert!(
            clients.select_peers(upstream.as_ref(), client_ip("198.51.100.10"), 1).peers.is_empty()
        );

        probe_upstream_peer(clients.clone(), metrics.clone(), upstream.clone(), peer.clone()).await;
        assert!(
            clients.select_peers(upstream.as_ref(), client_ip("198.51.100.10"), 1).peers.is_empty()
        );

        probe_upstream_peer(clients.clone(), metrics.clone(), upstream.clone(), peer).await;
        assert_eq!(
            clients.select_peers(upstream.as_ref(), client_ip("198.51.100.10"), 1).peers.len(),
            1
        );
        let rendered = metrics.render_prometheus();
        assert!(rendered.contains("rginx_active_health_checks_total"));
    }

    fn upstream_settings(protocol: UpstreamProtocol) -> UpstreamSettings {
        UpstreamSettings {
            protocol,
            load_balance: UpstreamLoadBalance::RoundRobin,
            server_name_override: None,
            request_timeout: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(30),
            write_timeout: Duration::from_secs(30),
            idle_timeout: Duration::from_secs(30),
            pool_idle_timeout: Some(Duration::from_secs(90)),
            pool_max_idle_per_host: usize::MAX,
            tcp_keepalive: None,
            tcp_nodelay: false,
            http2_keep_alive_interval: None,
            http2_keep_alive_timeout: Duration::from_secs(20),
            http2_keep_alive_while_idle: false,
            max_replayable_request_body_bytes: 64 * 1024,
            unhealthy_after_failures: 2,
            unhealthy_cooldown: Duration::from_secs(10),
            active_health_check: None,
        }
    }

    fn peer(url: &str) -> UpstreamPeer {
        peer_with_weight(url, 1)
    }

    fn peer_with_weight(url: &str, weight: u32) -> UpstreamPeer {
        peer_with_role(url, weight, false)
    }

    fn peer_with_role(url: &str, weight: u32, backup: bool) -> UpstreamPeer {
        let uri: http::Uri = url.parse().expect("peer URL should parse");
        UpstreamPeer {
            url: url.to_string(),
            scheme: uri.scheme_str().expect("peer should have scheme").to_string(),
            authority: uri.authority().expect("peer should have authority").to_string(),
            weight,
            backup,
        }
    }

    fn snapshot_with_upstream(name: &str, upstream: Arc<Upstream>) -> rginx_core::ConfigSnapshot {
        rginx_core::ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings {
                shutdown_timeout: Duration::from_secs(1),
                worker_threads: None,
                accept_workers: 1,
            },
            server: rginx_core::Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                request_body_read_timeout: None,
                response_write_timeout: None,
                access_log_format: None,
                tls: None,
            },
            default_vhost: rginx_core::VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            upstreams: HashMap::from([(name.to_string(), upstream)]),
        }
    }

    fn snapshot_with_upstream_policy(
        name: &str,
        peers: Vec<UpstreamPeer>,
        unhealthy_after_failures: u32,
        unhealthy_cooldown: Duration,
    ) -> rginx_core::ConfigSnapshot {
        let upstream = Upstream::new(
            name.to_string(),
            peers,
            UpstreamTls::NativeRoots,
            UpstreamSettings {
                unhealthy_after_failures,
                unhealthy_cooldown,
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        );

        rginx_core::ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings {
                shutdown_timeout: Duration::from_secs(1),
                worker_threads: None,
                accept_workers: 1,
            },
            server: rginx_core::Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                request_body_read_timeout: None,
                response_write_timeout: None,
                access_log_format: None,
                tls: None,
            },
            default_vhost: rginx_core::VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            upstreams: HashMap::from([(name.to_string(), Arc::new(upstream))]),
        }
    }

    fn snapshot_with_active_health(
        name: &str,
        peers: Vec<UpstreamPeer>,
        path: &str,
        healthy_successes_required: u32,
    ) -> rginx_core::ConfigSnapshot {
        let upstream = Upstream::new(
            name.to_string(),
            peers,
            UpstreamTls::NativeRoots,
            UpstreamSettings {
                active_health_check: Some(ActiveHealthCheck {
                    path: path.to_string(),
                    grpc_service: None,
                    interval: Duration::from_secs(5),
                    timeout: Duration::from_secs(1),
                    healthy_successes_required,
                }),
                ..upstream_settings(UpstreamProtocol::Auto)
            },
        );

        rginx_core::ConfigSnapshot {
            runtime: rginx_core::RuntimeSettings {
                shutdown_timeout: Duration::from_secs(1),
                worker_threads: None,
                accept_workers: 1,
            },
            server: rginx_core::Server {
                listen_addr: "127.0.0.1:8080".parse().unwrap(),
                trusted_proxies: Vec::new(),
                keep_alive: true,
                max_headers: None,
                max_request_body_bytes: None,
                max_connections: None,
                header_read_timeout: None,
                request_body_read_timeout: None,
                response_write_timeout: None,
                access_log_format: None,
                tls: None,
            },
            default_vhost: rginx_core::VirtualHost {
                id: "server".to_string(),
                server_names: Vec::new(),
                routes: Vec::new(),
                tls: None,
            },
            vhosts: Vec::new(),
            upstreams: HashMap::from([(name.to_string(), Arc::new(upstream))]),
        }
    }

    fn client_ip(value: &str) -> IpAddr {
        value.parse().expect("client IP should parse")
    }

    fn grpc_health_response_body(serving_status: u64) -> Bytes {
        let mut payload = BytesMut::new();
        payload.extend_from_slice(&[0x08]);
        if serving_status < 0x80 {
            payload.extend_from_slice(&[serving_status as u8]);
        } else {
            panic!("test serving status should fit in a single-byte protobuf varint");
        }

        let mut body = BytesMut::with_capacity(5 + payload.len());
        body.extend_from_slice(&[0]);
        body.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        body.extend_from_slice(&payload);
        body.freeze()
    }

    async fn spawn_status_server(statuses: Arc<Mutex<VecDeque<StatusCode>>>) -> SocketAddr {
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).expect("test status listener should bind");
        let listen_addr = listener.local_addr().expect("listener addr should exist");

        thread::spawn(move || {
            loop {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let statuses = statuses.clone();

                thread::spawn(move || {
                    let mut buffer = [0u8; 1024];
                    let _ = stream.read(&mut buffer);
                    let status = {
                        let mut statuses =
                            statuses.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        statuses.pop_front().unwrap_or(StatusCode::OK)
                    };
                    let reason = status.canonical_reason().unwrap_or("Unknown");
                    let response = format!(
                        "HTTP/1.1 {} {}\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                        status.as_u16(),
                        reason
                    );

                    let _ = stream.write_all(response.as_bytes());
                    let _ = stream.flush();
                });
            }
        });

        listen_addr
    }

    const TEST_CA_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIDXTCCAkWgAwIBAgIJAOIvDiVb18eVMA0GCSqGSIb3DQEBCwUAMEUxCzAJBgNV\nBAYTAkFVMRMwEQYDVQQIDApTb21lLVN0YXRlMSEwHwYDVQQKDBhJbnRlcm5ldCBX\naWRnaXRzIFB0eSBMdGQwHhcNMTYwODE0MTY1NjExWhcNMjYwODEyMTY1NjExWjBF\nMQswCQYDVQQGEwJBVTETMBEGA1UECAwKU29tZS1TdGF0ZTEhMB8GA1UECgwYSW50\nZXJuZXQgV2lkZ2l0cyBQdHkgTHRkMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIB\nCgKCAQEArVHWFn52Lbl1l59exduZntVSZyDYpzDND+S2LUcO6fRBWhV/1Kzox+2G\nZptbuMGmfI3iAnb0CFT4uC3kBkQQlXonGATSVyaFTFR+jq/lc0SP+9Bd7SBXieIV\neIXlY1TvlwIvj3Ntw9zX+scTA4SXxH6M0rKv9gTOub2vCMSHeF16X8DQr4XsZuQr\n7Cp7j1I4aqOJyap5JTl5ijmG8cnu0n+8UcRlBzy99dLWJG0AfI3VRJdWpGTNVZ92\naFff3RpK3F/WI2gp3qV1ynRAKuvmncGC3LDvYfcc2dgsc1N6Ffq8GIrkgRob6eBc\nklDHp1d023Lwre+VaVDSo1//Y72UFwIDAQABo1AwTjAdBgNVHQ4EFgQUbNOlA6sN\nXyzJjYqciKeId7g3/ZowHwYDVR0jBBgwFoAUbNOlA6sNXyzJjYqciKeId7g3/Zow\nDAYDVR0TBAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAVVaR5QWLZIRR4Dw6TSBn\nBQiLpBSXN6oAxdDw6n4PtwW6CzydaA+creiK6LfwEsiifUfQe9f+T+TBSpdIYtMv\nZ2H2tjlFX8VrjUFvPrvn5c28CuLI0foBgY8XGSkR2YMYzWw2jPEq3Th/KM5Catn3\nAFm3bGKWMtGPR4v+90chEN0jzaAmJYRrVUh9vea27bOCn31Nse6XXQPmSI6Gyncy\nOAPUsvPClF3IjeL1tmBotWqSGn1cYxLo+Lwjk22A9h6vjcNQRyZF2VLVvtwYrNU3\nmwJ6GCLsLHpwW/yjyvn8iEltnJvByM/eeRnfXV6WDObyiZsE/n6DxIRJodQzFqy9\nGA==\n-----END CERTIFICATE-----\n";
}
