use super::*;

pub(crate) struct TestServer {
    inner: ServerHarness,
}

impl TestServer {
    pub(crate) fn spawn(listen_addr: SocketAddr, config: String) -> Self {
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

    pub(crate) fn wait_for_http_ready(&mut self, listen_addr: SocketAddr, timeout: Duration) {
        self.inner.wait_for_http_ready(listen_addr, timeout);
    }

    pub(crate) fn wait_for_https_ready(&mut self, listen_addr: SocketAddr, timeout: Duration) {
        self.inner.wait_for_https_ready(listen_addr, timeout);
    }

    pub(crate) fn shutdown_and_wait(&mut self, timeout: Duration) {
        self.inner.shutdown_and_wait(timeout);
    }
}
pub(crate) fn https_h2_connector()
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

pub(crate) async fn wait_for_log_contains(server: &TestServer, timeout: Duration, needle: &str) {
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
