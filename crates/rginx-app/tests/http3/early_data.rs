use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn routes_http3_early_data_by_replay_safety() {
    let cert = generate_cert("localhost");
    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn_with_tls(
        "rginx-http3-early-data",
        &cert.cert.pem(),
        &cert.signing_key.serialize_pem(),
        |_, cert_path, key_path| http3_early_data_config(listen_addr, cert_path, key_path),
    );
    server.wait_for_https_ready(listen_addr, Duration::from_secs(5));
    wait_for_admin_socket(
        &rginx_runtime::admin::admin_socket_path_for_config(server.config_path()),
        Duration::from_secs(5),
    );

    let endpoint = http3_client_endpoint(None, &cert.cert.pem(), true)
        .expect("http3 early-data client should build");

    let (warmup, _) = http3_request_with_endpoint(
        &endpoint,
        listen_addr,
        "localhost",
        "GET",
        "/safe",
        &[],
        None,
        false,
        Duration::from_millis(150),
    )
    .await
    .expect("warmup request should succeed");
    assert_eq!(warmup.status, StatusCode::OK);
    assert_eq!(body_text(&warmup), "early data safe\n");

    let (safe, safe_accepted) = wait_for_http3_0rtt_request(
        &endpoint,
        listen_addr,
        "localhost",
        "/safe",
        Duration::from_secs(2),
    )
    .await
    .expect("0-RTT request to replay-safe route should succeed");
    assert!(safe_accepted, "server should accept 0-RTT data");
    assert_eq!(safe.status, StatusCode::OK);
    assert_eq!(body_text(&safe), "early data safe\n");

    let (unsafe_route, unsafe_accepted) = wait_for_http3_0rtt_request_status(
        &endpoint,
        listen_addr,
        "localhost",
        "/unsafe",
        StatusCode::TOO_EARLY,
        Duration::from_secs(2),
    )
    .await
    .expect("0-RTT request to non-replay-safe route should respond");
    assert!(unsafe_accepted, "server should keep 0-RTT enabled for the listener");
    assert_eq!(unsafe_route.status, StatusCode::TOO_EARLY);
    assert_eq!(body_text(&unsafe_route), "too early\n");

    endpoint.close(quinn::VarInt::from_u32(0), b"done");

    let status_output = run_rginx(["--config", server.config_path().to_str().unwrap(), "status"]);
    assert!(
        status_output.status.success(),
        "status command should succeed: {}",
        render_output(&status_output)
    );
    let status_stdout = String::from_utf8_lossy(&status_output.stdout);
    assert_eq!(parse_flat_u64(&status_stdout, "http3_early_data_enabled_listeners"), 1);
    assert!(parse_flat_u64(&status_stdout, "http3_early_data_rejected_requests") >= 1);

    let counters_output =
        run_rginx(["--config", server.config_path().to_str().unwrap(), "counters"]);
    assert!(
        counters_output.status.success(),
        "counters command should succeed: {}",
        render_output(&counters_output)
    );
    let counters_stdout = String::from_utf8_lossy(&counters_output.stdout);
    assert!(
        parse_flat_u64(&counters_stdout, "downstream_http3_early_data_rejected_requests_total")
            >= 1
    );

    server.shutdown_and_wait(Duration::from_secs(5));
}
