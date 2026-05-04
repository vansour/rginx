#![allow(unused_imports)]

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::io::{Read, Write};
use std::net::IpAddr;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use futures_util::stream;
use http::header::{CONTENT_LENGTH, CONTENT_TYPE, HOST, TE};
use http::{HeaderMap, HeaderValue, Method, Response, StatusCode, Uri, Version};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use rcgen::{CertifiedKey, KeyPair};
use rginx_core::{
    ActiveHealthCheck, ClientIdentity, Error, ProxyHeaderTemplate, ProxyHeaderValue, TlsVersion,
    Upstream, UpstreamDnsPolicy, UpstreamLoadBalance, UpstreamPeer, UpstreamProtocol,
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
    ResolvedUpstreamPeer, build_proxy_uri, probe_upstream_peer,
    remove_redundant_host_header_for_authority_pseudo_header, sanitize_request_headers,
    upstream_request_version,
};
use crate::client_ip::{ClientAddress, ClientIpSource};
use tempfile::TempDir;

mod cache;
mod cache_stale;
mod client_profiles;
mod grpc;
mod peer_recovery;
mod peer_selection;
mod request_headers;
mod support;

use support::spawn_range_server;

fn upstream_settings(protocol: UpstreamProtocol) -> UpstreamSettings {
    UpstreamSettings {
        protocol,
        load_balance: UpstreamLoadBalance::RoundRobin,
        dns: UpstreamDnsPolicy::default(),
        server_name: true,
        server_name_override: None,
        tls_versions: None,
        server_verify_depth: None,
        server_crl_path: None,
        client_identity: None,
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
        max_conns: None,
    }
}

fn resolved_peer_from_url(url: &str) -> ResolvedUpstreamPeer {
    let uri: http::Uri = url.parse().expect("peer URL should parse");
    let scheme = uri.scheme_str().expect("peer should have scheme").to_string();
    let authority = uri.authority().expect("peer should have authority").to_string();
    let host = uri.host().expect("peer should have host").to_string();
    let port = uri.port_u16().unwrap_or_else(|| if scheme == "https" { 443 } else { 80 });
    let socket_addr = host
        .parse::<IpAddr>()
        .map(|ip| SocketAddr::new(ip, port))
        .unwrap_or_else(|_| SocketAddr::new("127.0.0.1".parse().unwrap(), port));

    ResolvedUpstreamPeer {
        url: url.to_string(),
        logical_peer_url: url.to_string(),
        endpoint_key: url.to_string(),
        display_url: format!("{scheme}://{authority}"),
        scheme,
        upstream_authority: authority.clone(),
        dial_authority: authority,
        socket_addr,
        server_name: host,
        weight: 1,
        backup: false,
        max_conns: None,
    }
}

fn resolved_peer(peer: &UpstreamPeer) -> ResolvedUpstreamPeer {
    let mut resolved = resolved_peer_from_url(&peer.url);
    resolved.url = peer.url.clone();
    resolved.logical_peer_url = peer.url.clone();
    resolved.endpoint_key = peer.url.clone();
    resolved.display_url = peer.url.clone();
    resolved.upstream_authority = peer.authority.clone();
    resolved.dial_authority = peer.authority.clone();
    resolved.weight = peer.weight;
    resolved.backup = peer.backup;
    resolved.max_conns = peer.max_conns;
    resolved
}

async fn select(
    clients: &ProxyClients,
    upstream: &Upstream,
    client_ip: IpAddr,
    limit: usize,
) -> super::health::SelectedPeers {
    clients.select_peers(upstream, client_ip, limit).await
}

fn default_server() -> rginx_core::Server {
    rginx_core::Server {
        listen_addr: "127.0.0.1:8080".parse().unwrap(),
        server_header: rginx_core::default_server_header(),
        default_certificate: None,
        trusted_proxies: Vec::new(),
        client_ip_header: None,
        keep_alive: true,
        max_headers: None,
        max_request_body_bytes: None,
        max_connections: None,
        header_read_timeout: None,
        request_body_read_timeout: None,
        response_write_timeout: None,
        access_log_format: None,
        tls: None,
    }
}

fn default_listener(server: rginx_core::Server) -> rginx_core::Listener {
    rginx_core::Listener {
        id: "default".to_string(),
        name: "default".to_string(),
        server,
        tls_termination_enabled: false,
        proxy_protocol_enabled: false,
        http3: None,
    }
}

fn default_vhost() -> rginx_core::VirtualHost {
    rginx_core::VirtualHost {
        id: "server".to_string(),
        server_names: Vec::new(),
        routes: Vec::new(),
        tls: None,
    }
}

fn snapshot_with_upstreams_map(
    upstreams: HashMap<String, Arc<Upstream>>,
) -> rginx_core::ConfigSnapshot {
    let server = default_server();
    rginx_core::ConfigSnapshot {
        acme: None,
        managed_certificates: Vec::new(),
        cache_zones: HashMap::new(),
        runtime: rginx_core::RuntimeSettings {
            shutdown_timeout: Duration::from_secs(1),
            worker_threads: None,
            accept_workers: 1,
        },
        listeners: vec![default_listener(server)],
        default_vhost: default_vhost(),
        vhosts: Vec::new(),
        upstreams,
    }
}

fn snapshot_with_upstream(name: &str, upstream: Arc<Upstream>) -> rginx_core::ConfigSnapshot {
    snapshot_with_upstreams_map(HashMap::from([(name.to_string(), upstream)]))
}

fn snapshot_with_upstreams(
    upstreams: impl IntoIterator<Item = (String, Arc<Upstream>)>,
) -> rginx_core::ConfigSnapshot {
    snapshot_with_upstreams_map(HashMap::from_iter(upstreams))
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
    snapshot_with_upstreams_map(HashMap::from([(name.to_string(), Arc::new(upstream))]))
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
    snapshot_with_upstreams_map(HashMap::from([(name.to_string(), Arc::new(upstream))]))
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

struct StatusServerHandle {
    listen_addr: SocketAddr,
    shutdown: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for StatusServerHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

async fn spawn_status_server(statuses: Arc<Mutex<VecDeque<StatusCode>>>) -> StatusServerHandle {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test status listener should bind");
    listener.set_nonblocking(true).expect("status listener should support nonblocking mode");
    let listen_addr = listener.local_addr().expect("listener addr should exist");
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_thread = shutdown.clone();

    let thread = thread::spawn(move || {
        while !shutdown_thread.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let statuses = statuses.clone();

                    thread::spawn(move || {
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                        let mut buffer = [0u8; 1024];
                        match stream.read(&mut buffer) {
                            Ok(_) => {}
                            Err(error)
                                if matches!(
                                    error.kind(),
                                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                                ) =>
                            {
                                return;
                            }
                            Err(_) => return,
                        }
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

                        if stream.write_all(response.as_bytes()).is_err() {
                            return;
                        }
                        let _ = stream.flush();
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });

    StatusServerHandle { listen_addr, shutdown, thread: Some(thread) }
}

const TEST_CA_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIDXTCCAkWgAwIBAgIJAOIvDiVb18eVMA0GCSqGSIb3DQEBCwUAMEUxCzAJBgNV\nBAYTAkFVMRMwEQYDVQQIDApTb21lLVN0YXRlMSEwHwYDVQQKDBhJbnRlcm5ldCBX\naWRnaXRzIFB0eSBMdGQwHhcNMTYwODE0MTY1NjExWhcNMjYwODEyMTY1NjExWjBF\nMQswCQYDVQQGEwJBVTETMBEGA1UECAwKU29tZS1TdGF0ZTEhMB8GA1UECgwYSW50\nZXJuZXQgV2lkZ2l0cyBQdHkgTHRkMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIB\nCgKCAQEArVHWFn52Lbl1l59exduZntVSZyDYpzDND+S2LUcO6fRBWhV/1Kzox+2G\nZptbuMGmfI3iAnb0CFT4uC3kBkQQlXonGATSVyaFTFR+jq/lc0SP+9Bd7SBXieIV\neIXlY1TvlwIvj3Ntw9zX+scTA4SXxH6M0rKv9gTOub2vCMSHeF16X8DQr4XsZuQr\n7Cp7j1I4aqOJyap5JTl5ijmG8cnu0n+8UcRlBzy99dLWJG0AfI3VRJdWpGTNVZ92\naFff3RpK3F/WI2gp3qV1ynRAKuvmncGC3LDvYfcc2dgsc1N6Ffq8GIrkgRob6eBc\nklDHp1d023Lwre+VaVDSo1//Y72UFwIDAQABo1AwTjAdBgNVHQ4EFgQUbNOlA6sN\nXyzJjYqciKeId7g3/ZowHwYDVR0jBBgwFoAUbNOlA6sNXyzJjYqciKeId7g3/Zow\nDAYDVR0TBAUwAwEB/zANBgkqhkiG9w0BAQsFAAOCAQEAVVaR5QWLZIRR4Dw6TSBn\nBQiLpBSXN6oAxdDw6n4PtwW6CzydaA+creiK6LfwEsiifUfQe9f+T+TBSpdIYtMv\nZ2H2tjlFX8VrjUFvPrvn5c28CuLI0foBgY8XGSkR2YMYzWw2jPEq3Th/KM5Catn3\nAFm3bGKWMtGPR4v+90chEN0jzaAmJYRrVUh9vea27bOCn31Nse6XXQPmSI6Gyncy\nOAPUsvPClF3IjeL1tmBotWqSGn1cYxLo+Lwjk22A9h6vjcNQRyZF2VLVvtwYrNU3\nmwJ6GCLsLHpwW/yjyvn8iEltnJvByM/eeRnfXV6WDObyiZsE/n6DxIRJodQzFqy9\nGA==\n-----END CERTIFICATE-----\n";

type TestCertifiedKey = CertifiedKey<KeyPair>;

fn write_test_identity(cert_path: &Path, key_path: &Path) {
    let identity = generate_test_identity("localhost");
    std::fs::write(cert_path, identity.cert.pem()).expect("test cert should be written");
    std::fs::write(key_path, identity.signing_key.serialize_pem())
        .expect("test key should be written");
}

fn generate_test_identity(hostname: &str) -> TestCertifiedKey {
    rcgen::generate_simple_self_signed(vec![hostname.to_string()])
        .expect("self-signed certificate should generate")
}
