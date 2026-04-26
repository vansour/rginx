use std::env;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use rustls::ClientConfig;
use rustls::RootCertStore;

use super::*;

fn temp_dir(prefix: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    let path =
        env::temp_dir().join(format!("{prefix}-{}", NEXT_ID.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::create_dir_all(&path).expect("temp dir should be created");
    path
}

fn test_ocsp_client() -> OcspClient {
    rginx_http::install_default_crypto_provider();
    let tls_config = ClientConfig::builder()
        .with_root_certificates(RootCertStore::empty())
        .with_no_client_auth();
    let connector = HttpsConnectorBuilder::new()
        .with_tls_config(tls_config)
        .https_or_http()
        .enable_http1()
        .build();
    Client::builder(TokioExecutor::new()).build(connector)
}

fn spawn_http_responder(status: &str, body: Vec<u8>) -> (SocketAddr, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("test responder should bind");
    let listen_addr = listener.local_addr().expect("responder addr should exist");
    let status = status.to_string();

    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("request should connect");
        let mut buffer = [0_u8; 1024];
        let _ = stream.read(&mut buffer);
        let response = format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/ocsp-response\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).expect("response head should be written");
        stream.write_all(&body).expect("response body should be written");
        stream.flush().expect("response should flush");
    });

    (listen_addr, handle)
}

#[tokio::test]
async fn fetch_ocsp_response_from_url_rejects_non_success_status() {
    let client = test_ocsp_client();
    let (responder, responder_handle) =
        spawn_http_responder("500 Internal Server Error", b"fail".to_vec());

    let error =
        fetch_ocsp_response_from_url(&client, &format!("http://{responder}/ocsp"), vec![1, 2, 3])
            .await
            .expect_err("HTTP 500 should be rejected");

    assert!(error.contains("responder returned HTTP 500"));
    responder_handle.join().expect("responder thread should join");
}

#[tokio::test]
async fn fetch_ocsp_response_from_url_rejects_empty_body() {
    let client = test_ocsp_client();
    let (responder, responder_handle) = spawn_http_responder("200 OK", Vec::new());

    let error =
        fetch_ocsp_response_from_url(&client, &format!("http://{responder}/ocsp"), vec![4, 5, 6])
            .await
            .expect_err("empty OCSP body should be rejected");

    assert!(error.contains("empty OCSP response body"));
    responder_handle.join().expect("responder thread should join");
}

#[tokio::test]
async fn fetch_ocsp_response_tries_multiple_responders_until_one_succeeds() {
    let client = test_ocsp_client();
    let (first, first_handle) = spawn_http_responder("500 Internal Server Error", b"nope".to_vec());
    let (second, second_handle) = spawn_http_responder("200 OK", b"valid-ocsp-response".to_vec());

    let response = fetch_ocsp_response(
        &client,
        &[format!("http://{first}/ocsp"), format!("http://{second}/ocsp")],
        vec![7, 8, 9],
    )
    .await
    .expect("second responder should succeed");

    assert_eq!(response, b"valid-ocsp-response".to_vec());
    first_handle.join().expect("first responder thread should join");
    second_handle.join().expect("second responder thread should join");
}

#[tokio::test]
async fn write_ocsp_cache_file_creates_parent_directory_and_skips_unchanged_writes() {
    let temp_dir = temp_dir("rginx-runtime-ocsp-cache");
    let cache_path = temp_dir.join("nested").join("server.ocsp");

    let first_write = write_ocsp_cache_file(&cache_path, b"fresh-ocsp-response")
        .await
        .expect("initial write should succeed");
    let second_write = write_ocsp_cache_file(&cache_path, b"fresh-ocsp-response")
        .await
        .expect("unchanged write should succeed");

    assert!(first_write);
    assert!(!second_write);
    assert_eq!(
        tokio::fs::read(&cache_path).await.expect("cache file should be readable"),
        b"fresh-ocsp-response"
    );

    let _ = std::fs::remove_dir_all(temp_dir);
}

#[tokio::test]
async fn handle_ocsp_refresh_failure_clears_oversized_cache_file() {
    let temp_dir = temp_dir("rginx-runtime-ocsp-clear");
    let cache_path = temp_dir.join("server.ocsp");
    tokio::fs::write(&cache_path, vec![0_u8; rginx_http::MAX_OCSP_RESPONSE_BYTES + 1])
        .await
        .expect("oversized cache should be written");

    let (message, cache_cleared) = handle_ocsp_refresh_failure(
        Path::new("/tmp/unused-cert.pem"),
        &cache_path,
        rginx_core::OcspResponderPolicy::IssuerOrDelegated,
        "refresh failed".to_string(),
    )
    .await;

    assert!(cache_cleared);
    assert!(message.contains("refresh failed"));
    assert!(message.contains("cleared stale OCSP cache"));
    assert_eq!(
        tokio::fs::read(&cache_path).await.expect("cleared cache should be readable"),
        Vec::<u8>::new()
    );

    let _ = std::fs::remove_dir_all(temp_dir);
}
