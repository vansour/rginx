use super::*;
use std::sync::Arc;

use bytes::{Buf, BytesMut};
use h3::client;
use hyper::http::Request;
use quinn::crypto::rustls::QuicClientConfig;
use rustls::pki_types::{CertificateDer, pem::PemObject};
use rustls::{ClientConfig, RootCertStore};

#[test]
fn sighup_reload_applies_updated_routes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "before reload\n");

    server.wait_for_body(listen_addr, "before reload\n", Duration::from_secs(5));

    server.write_return_config(listen_addr, "after reload\n");
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(listen_addr, "after reload\n", Duration::from_secs(5));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn nginx_style_reload_command_applies_updated_routes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "before reload\n");

    server.wait_for_body(listen_addr, "before reload\n", Duration::from_secs(5));

    server.write_return_config(listen_addr, "after reload\n");
    let output = server.send_cli_signal("reload");

    assert!(output.status.success(), "rginx -s reload should succeed: {}", render_output(&output));

    server.wait_for_body(listen_addr, "after reload\n", Duration::from_secs(5));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn nginx_style_quit_command_stops_the_server() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "before quit\n");

    server.wait_for_body(listen_addr, "before quit\n", Duration::from_secs(5));

    let output = server.send_cli_signal("quit");
    assert!(output.status.success(), "rginx -s quit should succeed: {}", render_output(&output));

    let status = server.wait_for_exit(Duration::from_secs(5));
    assert!(status.success(), "rginx should exit cleanly after quit: {status}");
}

#[test]
fn sighup_reload_adds_explicit_listener_without_restart() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let http_addr = reserve_loopback_addr();
    let admin_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn_with_config(
        "rginx-reload-add-listener",
        explicit_listeners_config(&[("http", http_addr)], "before add\n"),
    );

    server.wait_for_body(http_addr, "before add\n", Duration::from_secs(5));
    assert_unreachable(admin_addr, Duration::from_millis(500));

    server.write_config(explicit_listeners_config(
        &[("http", http_addr), ("admin", admin_addr)],
        "after add\n",
    ));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(http_addr, "after add\n", Duration::from_secs(5));
    server.wait_for_body(admin_addr, "after add\n", Duration::from_secs(5));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_reload_removes_explicit_listener_without_restart() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let http_addr = reserve_loopback_addr();
    let admin_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn_with_config(
        "rginx-reload-remove-listener",
        explicit_listeners_config(&[("http", http_addr), ("admin", admin_addr)], "before remove\n"),
    );

    server.wait_for_body(http_addr, "before remove\n", Duration::from_secs(5));
    server.wait_for_body(admin_addr, "before remove\n", Duration::from_secs(5));

    server.write_config(explicit_listeners_config(&[("http", http_addr)], "after remove\n"));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(http_addr, "after remove\n", Duration::from_secs(5));
    assert_unreachable(admin_addr, Duration::from_secs(2));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn removed_listener_drains_in_flight_request_before_going_unreachable() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let http_addr = reserve_loopback_addr();
    let drain_addr = reserve_loopback_addr();
    let (ready_tx, ready_rx) = mpsc::channel();
    let upstream_addr =
        spawn_delayed_response_server(Duration::from_millis(300), "draining\n", Some(ready_tx));
    let mut server = TestServer::spawn_with_config(
        "rginx-reload-drain-listener",
        explicit_listeners_proxy_config(
            &[("http", http_addr), ("drain", drain_addr)],
            upstream_addr,
        ),
    );

    server.wait_for_body(http_addr, "draining\n", Duration::from_secs(5));
    server.wait_for_body(drain_addr, "draining\n", Duration::from_secs(5));
    while ready_rx.try_recv().is_ok() {}

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        tx.send(fetch_text_response_with_timeout(drain_addr, "/", Duration::from_secs(3)))
            .expect("result channel should remain available");
    });
    ready_rx.recv_timeout(Duration::from_secs(5)).expect("in-flight request should reach upstream");

    server.write_config(explicit_listeners_proxy_config(&[("http", http_addr)], upstream_addr));
    server.send_signal(libc::SIGHUP);

    server.wait_for_body(http_addr, "draining\n", Duration::from_secs(5));
    let result = rx.recv_timeout(Duration::from_secs(5)).expect("in-flight request should finish");
    let (status, body) = result.expect("in-flight request should succeed");
    assert_eq!(status, 200);
    assert_eq!(body, "draining\n");

    assert_unreachable(drain_addr, Duration::from_secs(2));
    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn removed_http3_listener_drains_in_flight_request_before_going_unreachable() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let cert = generate_cert("localhost");
    let http_addr = reserve_loopback_addr();
    let h3_addr = reserve_loopback_addr();
    let (ready_tx, ready_rx) = mpsc::channel();
    let upstream_addr =
        spawn_delayed_response_server(Duration::from_millis(300), "draining\n", Some(ready_tx));
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-reload-drain-http3-listener",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| {
            explicit_http3_listener_proxy_config(
                http_addr,
                Some((h3_addr, cert_path, key_path)),
                upstream_addr,
            )
        },
    );

    server.wait_for_http_ready(http_addr, Duration::from_secs(5));
    server.wait_for_https_ready(h3_addr, Duration::from_secs(5));
    wait_for_http3_body(
        h3_addr,
        "localhost",
        "/-/ready",
        &cert.cert.pem(),
        "ready\n",
        Duration::from_secs(5),
    );
    while ready_rx.try_recv().is_ok() {}

    let (tx, rx) = mpsc::channel();
    let cert_pem = cert.cert.pem();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime should build");
        let result =
            runtime.block_on(async { http3_get_body(h3_addr, "localhost", "/", &cert_pem).await });
        tx.send(result).expect("result channel should remain available");
    });
    ready_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("in-flight http3 request should reach upstream");

    fs::write(
        server.config_path(),
        explicit_http3_listener_proxy_config(http_addr, None, upstream_addr),
    )
    .expect("reloaded config should be written");
    server.send_signal(libc::SIGHUP);

    server.wait_for_http_text_response(
        http_addr,
        &http_addr.to_string(),
        "/",
        200,
        "draining\n",
        Duration::from_secs(5),
    );
    let result =
        rx.recv_timeout(Duration::from_secs(5)).expect("in-flight http3 request should finish");
    let body = result.expect("in-flight http3 request should succeed");
    assert_eq!(body, "draining\n");

    assert_http3_unreachable(
        h3_addr,
        "localhost",
        "/-/ready",
        &cert.cert.pem(),
        Duration::from_secs(3),
    );
    server.shutdown_and_wait(Duration::from_secs(5));
}

fn wait_for_http3_body(
    listen_addr: SocketAddr,
    server_name: &str,
    path: &str,
    cert_pem: &str,
    expected: &str,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    let mut last_error = String::new();

    while Instant::now() < deadline {
        match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime should build")
            .block_on(async { http3_get_body(listen_addr, server_name, path, cert_pem).await })
        {
            Ok(body) if body == expected => return,
            Ok(body) => last_error = format!("unexpected body {body:?}"),
            Err(error) => last_error = error,
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    panic!(
        "timed out waiting for expected http3 response on {}; expected body {:?}; last error: {}",
        listen_addr, expected, last_error
    );
}

fn assert_http3_unreachable(
    listen_addr: SocketAddr,
    server_name: &str,
    path: &str,
    cert_pem: &str,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("tokio runtime should build")
            .block_on(async { http3_get_body(listen_addr, server_name, path, cert_pem).await });
        if let Ok(body) = result {
            panic!(
                "expected http3 listener {} to stay unreachable, got body {:?}",
                listen_addr, body
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

async fn http3_get_body(
    listen_addr: SocketAddr,
    server_name: &str,
    path: &str,
    cert_pem: &str,
) -> Result<String, String> {
    let roots = root_store_from_pem(cert_pem)?;
    let mut client_crypto = ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::aws_lc_rs::default_provider(),
    ))
    .with_protocol_versions(&[&rustls::version::TLS13])
    .map_err(|error| format!("failed to constrain TLS versions for http3 client: {error}"))?
    .with_root_certificates(roots)
    .with_no_client_auth();
    client_crypto.alpn_protocols = vec![b"h3".to_vec()];

    let client_config = quinn::ClientConfig::new(Arc::new(
        QuicClientConfig::try_from(client_crypto)
            .map_err(|error| format!("failed to build quic client config: {error}"))?,
    ));
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap())
        .map_err(|error| error.to_string())?;
    endpoint.set_default_client_config(client_config);

    let connection = endpoint
        .connect(listen_addr, server_name)
        .map_err(|error| format!("failed to start quic connect: {error}"))?
        .await
        .map_err(|error| format!("quic connect failed: {error}"))?;

    let (mut driver, mut send_request) =
        client::new(h3_quinn::Connection::new(connection))
            .await
            .map_err(|error| format!("failed to initialize http3 client: {error}"))?;
    let mut driver_task = tokio::spawn(async move {
        let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
    });

    let request = Request::builder()
        .method("GET")
        .uri(format!("https://{server_name}:{}{path}", listen_addr.port()))
        .body(())
        .expect("http3 request should build");
    let mut request_stream = send_request
        .send_request(request)
        .await
        .map_err(|error| format!("failed to send http3 request: {error}"))?;
    request_stream
        .finish()
        .await
        .map_err(|error| format!("failed to finish http3 request: {error}"))?;

    let response = request_stream
        .recv_response()
        .await
        .map_err(|error| format!("failed to receive http3 response headers: {error}"))?;
    if response.status().as_u16() != 200 {
        return Err(format!("unexpected http3 status {}", response.status().as_u16()));
    }

    let mut body = BytesMut::new();
    while let Some(chunk) = request_stream
        .recv_data()
        .await
        .map_err(|error| format!("failed to receive http3 response body: {error}"))?
    {
        body.extend_from_slice(chunk.chunk());
    }
    let _ = request_stream
        .recv_trailers()
        .await
        .map_err(|error| format!("failed to receive http3 response trailers: {error}"))?;

    if tokio::time::timeout(Duration::from_millis(50), &mut driver_task).await.is_err() {
        driver_task.abort();
        let _ = driver_task.await;
    }
    endpoint.close(quinn::VarInt::from_u32(0), b"done");
    String::from_utf8(body.to_vec()).map_err(|error| format!("http3 body was not utf-8: {error}"))
}

fn root_store_from_pem(cert_pem: &str) -> Result<RootCertStore, String> {
    let cert = CertificateDer::pem_slice_iter(cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to parse certificate PEM: {error}"))?
        .into_iter()
        .next()
        .ok_or_else(|| "certificate PEM did not contain a certificate".to_string())?;
    let mut roots = RootCertStore::empty();
    roots.add(cert).map_err(|error| format!("failed to add root certificate: {error}"))?;
    Ok(roots)
}

fn explicit_http3_listener_proxy_config(
    http_addr: SocketAddr,
    http3: Option<(SocketAddr, &Path, &Path)>,
    upstream_addr: SocketAddr,
) -> String {
    let mut listeners = vec![format!(
        "        ListenerConfig(\n            name: \"http\",\n            listen: {:?},\n        )",
        http_addr.to_string()
    )];
    if let Some((http3_addr, cert_path, key_path)) = http3 {
        listeners.push(format!(
            "        ListenerConfig(\n            name: \"https\",\n            listen: {:?},\n            tls: Some(ServerTlsConfig(\n                cert_path: {:?},\n                key_path: {:?},\n            )),\n            http3: Some(Http3Config(\n                advertise_alt_svc: Some(true),\n                alt_svc_max_age_secs: Some(7200),\n            )),\n        )",
            http3_addr.to_string(),
            cert_path.display().to_string(),
            key_path.display().to_string(),
        ));
    }
    let listeners = listeners.join(",\n");

    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    listeners: [\n{listeners}\n    ],\n    server: ServerConfig(\n        server_names: [\"localhost\"],\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {upstream:?},\n                ),\n            ],\n            request_timeout_secs: Some(3),\n        ),\n    ],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listeners = listeners,
        upstream = format!("http://{upstream_addr}"),
        ready_route = READY_ROUTE_CONFIG,
    )
}
