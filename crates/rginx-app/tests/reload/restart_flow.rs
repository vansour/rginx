use super::*;
use std::sync::Arc;

use bytes::{Buf, BytesMut};
use h3::client;
use hyper::http::Request;
use quinn::crypto::rustls::QuicClientConfig;
use rustls::pki_types::{CertificateDer, pem::PemObject};
use rustls::{ClientConfig, RootCertStore};

#[test]
fn nginx_style_restart_command_applies_listen_address_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let initial_addr = reserve_loopback_addr();
    let restarted_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(initial_addr, "before restart\n");

    server.wait_for_body(initial_addr, "before restart\n", Duration::from_secs(5));
    let old_pid = read_pid_file(&server.pid_path());

    server.write_return_config(restarted_addr, "after restart\n");
    let output = server.send_cli_signal("restart");
    assert!(output.status.success(), "rginx -s restart should succeed: {}", render_output(&output));

    let new_pid = wait_for_pid_change(&server.pid_path(), old_pid, Duration::from_secs(10));
    let status = server.wait_for_exit(Duration::from_secs(10));
    assert!(status.success(), "old process should exit cleanly after restart: {status}");

    wait_for_body(restarted_addr, "after restart\n", Duration::from_secs(10));
    assert_unreachable(initial_addr, Duration::from_millis(500));

    let quit = server.send_cli_signal("quit");
    assert!(
        quit.status.success(),
        "rginx -s quit should stop replacement process: {}",
        render_output(&quit)
    );
    wait_for_process_exit(new_pid, Duration::from_secs(10));
}

#[test]
fn nginx_style_restart_command_applies_runtime_worker_changes() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "runtime restart\n");

    server.wait_for_body(listen_addr, "runtime restart\n", Duration::from_secs(5));
    let old_pid = read_pid_file(&server.pid_path());

    server.write_config(return_config_with_runtime(
        listen_addr,
        "runtime restart\n",
        "        worker_threads: Some(2),\n        accept_workers: Some(2),\n",
    ));
    let output = server.send_cli_signal("restart");
    assert!(output.status.success(), "rginx -s restart should succeed: {}", render_output(&output));

    let new_pid = wait_for_pid_change(&server.pid_path(), old_pid, Duration::from_secs(10));
    let status = server.wait_for_exit(Duration::from_secs(10));
    assert!(status.success(), "old process should exit cleanly after restart: {status}");

    wait_for_body(listen_addr, "runtime restart\n", Duration::from_secs(10));
    let status_output = server.run_cli_command(["status"]);
    assert!(
        status_output.status.success(),
        "rginx status should succeed after restart: {}",
        render_output(&status_output)
    );
    let stdout = String::from_utf8_lossy(&status_output.stdout);
    assert!(stdout.contains("worker_threads=2"));
    assert!(stdout.contains("accept_workers=2"));

    let quit = server.send_cli_signal("quit");
    assert!(
        quit.status.success(),
        "rginx -s quit should stop replacement process: {}",
        render_output(&quit)
    );
    wait_for_process_exit(new_pid, Duration::from_secs(10));
}

#[test]
fn nginx_style_restart_command_keeps_old_process_running_when_replacement_fails() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn(listen_addr, "stable runtime\n");

    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));
    let old_pid = read_pid_file(&server.pid_path());

    server.write_config(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 0,\n    ),\n    server: ServerConfig(\n        listen: \"127.0.0.1:0\",\n    ),\n    upstreams: [],\n    locations: [],\n)\n".to_string(),
    );
    let output = server.send_cli_signal("restart");
    assert!(
        output.status.success(),
        "restart signal delivery should still succeed: {}",
        render_output(&output)
    );

    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(read_pid_file(&server.pid_path()), old_pid);
    server.wait_for_body(listen_addr, "stable runtime\n", Duration::from_secs(5));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn sighup_status_reports_tls_certificate_changes_after_rotation() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let listen_addr = reserve_loopback_addr();
    let initial_cert = generate_cert("localhost");
    let rotated_cert = generate_cert("localhost");
    let mut server = ServerHarness::spawn("rginx-reload-tls-rotation", |temp_dir| {
        let cert_path = temp_dir.join("server.crt");
        let key_path = temp_dir.join("server.key");
        fs::write(&cert_path, initial_cert.cert.pem()).expect("initial cert should be written");
        fs::write(&key_path, initial_cert.signing_key.serialize_pem())
            .expect("initial key should be written");
        tls_return_config(listen_addr, &cert_path, &key_path)
    });

    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

    let rotated_cert_path = server.temp_dir().join("server-rotated.crt");
    let rotated_key_path = server.temp_dir().join("server-rotated.key");
    fs::write(&rotated_cert_path, rotated_cert.cert.pem()).expect("rotated cert should be written");
    fs::write(&rotated_key_path, rotated_cert.signing_key.serialize_pem())
        .expect("rotated key should be written");
    fs::write(
        server.config_path(),
        tls_return_config(listen_addr, &rotated_cert_path, &rotated_key_path),
    )
    .expect("rotated TLS config should be written");
    server.send_signal(libc::SIGHUP);

    let stdout = wait_for_status_output(
        server.config_path(),
        |stdout| stdout.contains("reload_successes=1"),
        Duration::from_secs(5),
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));
    assert!(stdout.contains("reload_successes=1"), "stdout should report reload success: {stdout}");
    assert!(
        stdout.contains("last_reload_tls_certificate_changes=")
            && stdout.contains("listener:default:")
            && stdout.contains("->"),
        "stdout should report TLS certificate changes: {stdout}"
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn nginx_style_restart_command_keeps_http3_listener_available() {
    let _guard = test_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime should build")
        .block_on(async {
            let cert = generate_cert("localhost");
            let listen_addr = reserve_loopback_addr();
            let mut server = ServerHarness::spawn_with_tls(
                "rginx-restart-http3",
                &cert.cert.pem(),
                &cert.signing_key.serialize_pem(),
                |_, cert_path, key_path| {
                    http3_return_config(listen_addr, cert_path, key_path, "before restart\n")
                },
            );
            server.wait_for_https_ready(listen_addr, Duration::from_secs(5));

            let response = http3_get_body(listen_addr, "localhost", "/", &cert.cert.pem())
                .await
                .expect("http3 request before restart should succeed");
            assert_eq!(response, "before restart\n");

            let old_pid = read_pid_file(&server.config_path().with_extension("pid"));
            let cert_path = server.temp_dir().join("server.crt");
            let key_path = server.temp_dir().join("server.key");
            fs::write(
                server.config_path(),
                http3_return_config(listen_addr, &cert_path, &key_path, "after restart\n"),
            )
            .expect("updated http3 config should be written");

            let output = run_cli_command(server.config_path(), ["-s", "restart"]);
            assert!(
                output.status.success(),
                "rginx -s restart should succeed for http3 listeners: {}",
                render_output(&output)
            );

            let new_pid = wait_for_pid_change(
                &server.config_path().with_extension("pid"),
                old_pid,
                Duration::from_secs(10),
            );
            let status = server.wait_for_exit(Duration::from_secs(10));
            assert!(status.success(), "old process should exit cleanly after restart: {status}");

            let response = http3_get_body(listen_addr, "localhost", "/", &cert.cert.pem())
                .await
                .expect("http3 request after restart should succeed");
            assert_eq!(response, "after restart\n");

            let quit = run_cli_command(server.config_path(), ["-s", "quit"]);
            assert!(
                quit.status.success(),
                "rginx -s quit should stop replacement process: {}",
                render_output(&quit)
            );
            wait_for_process_exit(new_pid, Duration::from_secs(10));
        });
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
    let driver_task = tokio::spawn(async move {
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

    driver_task.abort();
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

fn http3_return_config(
    listen_addr: SocketAddr,
    cert_path: &Path,
    key_path: &Path,
    body: &str,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n        http3: Some(Http3Config(\n            advertise_alt_svc: Some(true),\n            alt_svc_max_age_secs: Some(7200),\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({body:?}),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
        body = body,
    )
}
