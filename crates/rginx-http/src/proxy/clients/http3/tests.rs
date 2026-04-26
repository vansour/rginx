use super::*;
use http::StatusCode;
use http_body_util::BodyExt;
use rginx_core::{UpstreamDnsPolicy, UpstreamLoadBalance, UpstreamSettings, UpstreamTls};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

fn test_resolver() -> Arc<UpstreamResolver> {
    Arc::new(UpstreamResolver::new(UpstreamDnsPolicy::default()).expect("resolver should build"))
}

#[tokio::test]
async fn reuses_cached_endpoint_for_repeated_ipv4_requests() {
    let client_config = super::super::tls::build_http3_client_config(
        &UpstreamTls::Insecure,
        None,
        None,
        None,
        None,
        true,
    )
    .expect("http3 client config should build");
    let client = Http3Client::new(client_config, Duration::from_secs(1), test_resolver());
    let remote_addr: SocketAddr = "127.0.0.1:443".parse().unwrap();

    assert_eq!(client.cached_endpoint_count().await, 0);

    let first = client
        .cached_endpoint_local_addr(remote_addr)
        .await
        .expect("first endpoint should be created");
    assert_eq!(client.cached_endpoint_count().await, 1);

    let second = client
        .cached_endpoint_local_addr(remote_addr)
        .await
        .expect("cached endpoint should be reused");
    assert_eq!(first, second);
    assert_eq!(client.cached_endpoint_count().await, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn reuses_http3_session_for_sequential_requests() {
    let client_config = super::super::tls::build_http3_client_config(
        &UpstreamTls::Insecure,
        None,
        None,
        None,
        None,
        true,
    )
    .expect("http3 client config should build");
    let resolver = test_resolver();
    let client = Http3Client::new(client_config, Duration::from_secs(1), resolver.clone());
    let accepted_connections = Arc::new(AtomicUsize::new(0));
    let request_count = Arc::new(AtomicUsize::new(0));
    let (listen_addr, server_task) =
        spawn_test_http3_server(accepted_connections.clone(), request_count.clone(), None);

    let upstream = Arc::new(Upstream::new(
        "backend".to_string(),
        vec![UpstreamPeer {
            url: format!("https://127.0.0.1:{}", listen_addr.port()),
            scheme: "https".to_string(),
            authority: format!("127.0.0.1:{}", listen_addr.port()),
            weight: 1,
            backup: false,
        }],
        UpstreamTls::Insecure,
        UpstreamSettings {
            protocol: UpstreamProtocol::Http3,
            load_balance: UpstreamLoadBalance::RoundRobin,
            dns: UpstreamDnsPolicy::default(),
            server_name: true,
            server_name_override: Some("localhost".to_string()),
            tls_versions: None,
            server_verify_depth: None,
            server_crl_path: None,
            client_identity: None,
            request_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(5),
            write_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(5),
            pool_idle_timeout: None,
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
        },
    ));
    let peer = resolver
        .resolve_peer(&upstream.peers[0])
        .await
        .expect("peer should resolve")
        .into_iter()
        .next()
        .expect("resolved peer should exist");

    let first = client
        .request(upstream.as_ref(), &peer, test_request(&peer.url, "/first"))
        .await
        .expect("first request should succeed");
    let first_body =
        first.into_body().collect().await.expect("first body should collect").to_bytes();
    assert_eq!(first_body, Bytes::from_static(b"ok\n"));

    let second = client
        .request(upstream.as_ref(), &peer, test_request(&peer.url, "/second"))
        .await
        .expect("second request should succeed");
    let second_body =
        second.into_body().collect().await.expect("second body should collect").to_bytes();
    assert_eq!(second_body, Bytes::from_static(b"ok\n"));

    assert_eq!(accepted_connections.load(Ordering::Relaxed), 1);
    assert_eq!(request_count.load(Ordering::Relaxed), 2);
    assert_eq!(client.cached_session_count().await, 1);

    server_task.abort();
    let _ = server_task.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn streams_http3_response_body_without_full_buffering() {
    let client_config = super::super::tls::build_http3_client_config(
        &UpstreamTls::Insecure,
        None,
        None,
        None,
        None,
        true,
    )
    .expect("http3 client config should build");
    let resolver = test_resolver();
    let client = Http3Client::new(client_config, Duration::from_secs(1), resolver.clone());
    let delay = Duration::from_millis(300);
    let (listen_addr, server_task) = spawn_test_http3_server(
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        Some(delay),
    );

    let upstream = Arc::new(Upstream::new(
        "backend".to_string(),
        vec![UpstreamPeer {
            url: format!("https://127.0.0.1:{}", listen_addr.port()),
            scheme: "https".to_string(),
            authority: format!("127.0.0.1:{}", listen_addr.port()),
            weight: 1,
            backup: false,
        }],
        UpstreamTls::Insecure,
        UpstreamSettings {
            protocol: UpstreamProtocol::Http3,
            load_balance: UpstreamLoadBalance::RoundRobin,
            dns: UpstreamDnsPolicy::default(),
            server_name: true,
            server_name_override: Some("localhost".to_string()),
            tls_versions: None,
            server_verify_depth: None,
            server_crl_path: None,
            client_identity: None,
            request_timeout: Duration::from_secs(5),
            connect_timeout: Duration::from_secs(5),
            write_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(5),
            pool_idle_timeout: None,
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
        },
    ));
    let peer = resolver
        .resolve_peer(&upstream.peers[0])
        .await
        .expect("peer should resolve")
        .into_iter()
        .next()
        .expect("resolved peer should exist");

    let started = Instant::now();
    let response = client
        .request(upstream.as_ref(), &peer, test_request(&peer.url, "/stream"))
        .await
        .expect("streaming request should succeed");
    assert!(started.elapsed() < delay);

    let mut body = response.into_body();
    let first = body
        .frame()
        .await
        .expect("first frame should exist")
        .expect("first frame should be ok")
        .into_data()
        .expect("first frame should contain data");
    assert_eq!(first, Bytes::from_static(b"part-1\n"));

    let second = body
        .frame()
        .await
        .expect("second frame should exist")
        .expect("second frame should be ok")
        .into_data()
        .expect("second frame should contain data");
    assert_eq!(second, Bytes::from_static(b"part-2\n"));

    server_task.abort();
    let _ = server_task.await;
}

fn test_request(base_url: &str, path: &str) -> Request<HttpBody> {
    Request::builder()
        .method("GET")
        .uri(format!("{base_url}{path}"))
        .body(boxed_body(http_body_util::Empty::<Bytes>::new()))
        .expect("request should build")
}

fn spawn_test_http3_server(
    accepted_connections: Arc<AtomicUsize>,
    request_count: Arc<AtomicUsize>,
    delayed_second_chunk: Option<Duration>,
) -> (SocketAddr, JoinHandle<()>) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("cert should generate");
    let cert_der = rustls::pki_types::CertificateDer::from(cert.cert);
    let key_der = rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der], key_der.into())
        .expect("server cert should configure");
    server_crypto.alpn_protocols = vec![b"h3".to_vec()];
    let server_config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)
            .expect("quic server config should build"),
    ));
    let endpoint = quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap()).unwrap();
    let listen_addr = endpoint.local_addr().expect("listen addr should exist");

    let task = tokio::spawn(async move {
        let incoming = endpoint.accept().await.expect("connection should arrive");
        accepted_connections.fetch_add(1, Ordering::Relaxed);
        let connection = incoming.await.expect("connection should establish");
        let mut h3 = h3::server::Connection::new(h3_quinn::Connection::new(connection))
            .await
            .expect("h3 server should initialize");

        while request_count.load(Ordering::Relaxed) < 2 || delayed_second_chunk.is_some() {
            let Some(resolver) = h3.accept().await.expect("h3 accept should succeed") else {
                break;
            };
            let (_request, mut stream) =
                resolver.resolve_request().await.expect("request should resolve");
            request_count.fetch_add(1, Ordering::Relaxed);
            stream
                .send_response(Response::builder().status(StatusCode::OK).body(()).unwrap())
                .await
                .expect("response headers should send");
            if let Some(delay) = delayed_second_chunk {
                stream
                    .send_data(Bytes::from_static(b"part-1\n"))
                    .await
                    .expect("first chunk should send");
                tokio::time::sleep(delay).await;
                stream
                    .send_data(Bytes::from_static(b"part-2\n"))
                    .await
                    .expect("second chunk should send");
                stream.finish().await.expect("stream should finish");
                tokio::time::sleep(Duration::from_millis(100)).await;
                break;
            } else {
                stream.send_data(Bytes::from_static(b"ok\n")).await.expect("body should send");
                stream.finish().await.expect("stream should finish");
                if request_count.load(Ordering::Relaxed) >= 2 {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    break;
                }
            }
        }
    });

    (listen_addr, task)
}
