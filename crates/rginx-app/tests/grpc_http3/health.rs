use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn active_grpc_health_checks_can_target_http3_upstreams() {
    let cert = generate_cert("localhost");
    let shared_dir = TempDirGuard::new("rginx-grpc-http3-health-shared");
    let server_cert_path = shared_dir.path().join("server.crt");
    let server_key_path = shared_dir.path().join("server.key");
    fs::write(&server_cert_path, cert.cert.pem()).expect("server cert should be written");
    fs::write(&server_key_path, cert.signing_key.serialize_pem())
        .expect("server key should be written");

    let (upstream_addr, health_seen_rx, upstream_task, _upstream_temp_dir) =
        spawn_h3_grpc_health_upstream(&server_cert_path, &server_key_path).await;

    let listen_addr = reserve_loopback_addr();
    let mut server = ServerHarness::spawn("rginx-grpc-http3-health", |_| {
        grpc_http3_health_config(listen_addr, upstream_addr)
    });
    server.wait_for_http_ready(listen_addr, Duration::from_secs(5));

    tokio::time::timeout(Duration::from_secs(5), health_seen_rx)
        .await
        .expect("health probe should arrive before timeout")
        .expect("health probe channel should complete");

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.abort();
    let _ = upstream_task.await;
}
