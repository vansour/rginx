use super::*;
use std::future::poll_fn;
use tokio::time::timeout;

#[tokio::test(flavor = "multi_thread")]
async fn clean_accept_close_detects_quinn_application_close_zero() {
    let (server_endpoint, client_endpoint, server_addr) = test_http3_endpoint_pair();
    let server_task = tokio::spawn(async move {
        let incoming = server_endpoint.accept().await.expect("connection should arrive");
        let connection = incoming.await.expect("connection should establish");
        let mut h3 =
            match H3Connection::<_, Bytes>::new(h3_quinn::Connection::new(connection)).await {
                Ok(h3) => h3,
                Err(error) => return error,
            };

        match h3.accept().await {
            Err(error) => error,
            Ok(Some(_resolver)) => {
                panic!("client application close should not produce a request")
            }
            Ok(None) => panic!("client application close should produce an h3 accept error"),
        }
    });

    let connection = client_endpoint
        .connect(server_addr, "localhost")
        .expect("client connect should start")
        .await
        .expect("client connection should establish");
    let close_connection = connection.clone();
    let (mut driver, _send_request) = h3::client::new(h3_quinn::Connection::new(connection))
        .await
        .expect("h3 client should initialize");
    let driver_task = tokio::spawn(async move {
        let _ = poll_fn(|cx| driver.poll_close(cx)).await;
    });

    close_connection.close(quinn::VarInt::from_u32(0), b"");
    let error = timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server should observe clean close")
        .expect("server task should not panic");

    assert!(
        is_clean_http3_accept_close(&error),
        "clean QUIC application close should be treated as debug-only, got {error}"
    );

    driver_task.abort();
    let _ = driver_task.await;
}

fn test_http3_endpoint_pair() -> (quinn::Endpoint, quinn::Endpoint, SocketAddr) {
    let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("cert should generate");
    let cert_der = rustls::pki_types::CertificateDer::from(cert.cert);
    let key_der = rustls::pki_types::PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());

    let mut server_crypto = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert_der.clone()], key_der.into())
        .expect("server cert should configure");
    server_crypto.alpn_protocols = vec![b"h3".to_vec()];
    let server_config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(server_crypto)
            .expect("quic server config should build"),
    ));
    let server_endpoint = quinn::Endpoint::server(server_config, "127.0.0.1:0".parse().unwrap())
        .expect("server endpoint should bind");
    let server_addr = server_endpoint.local_addr().expect("server addr should exist");

    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert_der).expect("client should trust test certificate");
    let mut client_crypto =
        rustls::ClientConfig::builder().with_root_certificates(roots).with_no_client_auth();
    client_crypto.alpn_protocols = vec![b"h3".to_vec()];
    let client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(client_crypto)
            .expect("quic client config should build"),
    ));
    let mut client_endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap())
        .expect("client endpoint should bind");
    client_endpoint.set_default_client_config(client_config);

    (server_endpoint, client_endpoint, server_addr)
}
