use super::*;

pub struct ServerHarness {
    child: Child,
    config_path: PathBuf,
    temp_dir: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl ServerHarness {
    pub fn spawn(prefix: &str, build_config: impl FnOnce(&Path) -> String) -> Self {
        Self::spawn_inner(prefix, move |temp_dir| build_config(temp_dir))
    }

    pub fn spawn_with_tls(
        prefix: &str,
        cert_pem: &str,
        key_pem: &str,
        build_config: impl FnOnce(&Path, &Path, &Path) -> String,
    ) -> Self {
        Self::spawn_inner(prefix, move |temp_dir| {
            let cert_path = temp_dir.join("server.crt");
            let key_path = temp_dir.join("server.key");
            fs::write(&cert_path, cert_pem).expect("test cert should be written");
            fs::write(&key_path, key_pem).expect("test key should be written");
            build_config(temp_dir, &cert_path, &key_path)
        })
    }

    fn spawn_inner(prefix: &str, build_config: impl FnOnce(&Path) -> String) -> Self {
        let temp_dir = temp_dir(prefix);
        fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
        let config_path = temp_dir.join("rginx.ron");
        fs::write(&config_path, build_config(&temp_dir)).expect("config should be written");
        let stdout_path = temp_dir.join("stdout.log");
        let stderr_path = temp_dir.join("stderr.log");
        let stdout = fs::File::create(&stdout_path).expect("stdout log file should be created");
        let stderr = fs::File::create(&stderr_path).expect("stderr log file should be created");

        let child = Command::new(binary_path())
            .arg("--config")
            .arg(&config_path)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("rginx should start");

        Self { child, config_path, temp_dir, stdout_path, stderr_path }
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    pub fn temp_dir(&self) -> &Path {
        &self.temp_dir
    }

    pub fn wait_for_http_ready(&mut self, listen_addr: SocketAddr, timeout: Duration) {
        self.wait_for_http_text_response(
            listen_addr,
            &listen_addr.to_string(),
            READY_PATH,
            200,
            READY_BODY,
            timeout,
        );
    }

    pub fn wait_for_https_ready(&mut self, listen_addr: SocketAddr, timeout: Duration) {
        self.wait_for_https_text_response(
            listen_addr,
            &listen_addr.to_string(),
            READY_PATH,
            DEFAULT_TLS_SERVER_NAME,
            200,
            READY_BODY,
            timeout,
        );
    }

    pub fn wait_for_http_text_response(
        &mut self,
        listen_addr: SocketAddr,
        host: &str,
        path: &str,
        expected_status: u16,
        expected_body: &str,
        timeout: Duration,
    ) {
        self.wait_for_text_response(
            timeout,
            || fetch_http_text_response(listen_addr, host, path),
            format!("http://{listen_addr}{path}"),
            expected_status,
            expected_body,
        );
    }

    pub fn wait_for_https_text_response(
        &mut self,
        listen_addr: SocketAddr,
        host: &str,
        path: &str,
        server_name: &str,
        expected_status: u16,
        expected_body: &str,
        timeout: Duration,
    ) {
        self.wait_for_text_response(
            timeout,
            || fetch_https_text_response(listen_addr, host, path, server_name),
            format!("https://{listen_addr}{path}"),
            expected_status,
            expected_body,
        );
    }

    fn wait_for_text_response(
        &mut self,
        timeout: Duration,
        mut fetch: impl FnMut() -> Result<(u16, String), String>,
        target: String,
        expected_status: u16,
        expected_body: &str,
    ) {
        let deadline = Instant::now() + timeout;
        let mut last_error = String::new();

        while Instant::now() < deadline {
            self.assert_running();

            match fetch() {
                Ok((status, body)) if status == expected_status && body == expected_body => {
                    self.assert_running();
                    return;
                }
                Ok((status, body)) => {
                    last_error =
                        format!("unexpected response from {target}: status={status} body={body:?}");
                }
                Err(error) => last_error = error,
            }

            thread::sleep(Duration::from_millis(50));
        }

        panic!(
            "timed out waiting for expected response on {target}; expected status={} body={:?}; last error: {}\n{}",
            expected_status,
            expected_body,
            last_error,
            self.combined_output()
        );
    }

    pub fn shutdown_and_wait(&mut self, timeout: Duration) {
        self.kill_and_wait(timeout);
    }

    pub fn kill_and_wait(&mut self, timeout: Duration) {
        self.child.kill().expect("rginx should accept a kill signal");
        let status = self.wait_for_exit(timeout);
        assert!(
            !status.success() || status.code() == Some(0),
            "rginx should exit after the test, got {status}\n{}",
            self.combined_output()
        );
    }

    #[cfg(unix)]
    pub fn send_signal(&self, signal: i32) {
        let result = unsafe { libc::kill(self.child.id() as i32, signal) };
        if result != 0 {
            panic!(
                "failed to send signal {} to pid {}: {}\n{}",
                signal,
                self.child.id(),
                std::io::Error::last_os_error(),
                self.combined_output()
            );
        }
    }

    #[cfg(unix)]
    pub fn terminate_and_wait(&mut self, timeout: Duration) {
        self.send_signal(libc::SIGTERM);
        let status = self.wait_for_exit(timeout);
        assert!(
            status.success(),
            "rginx should exit successfully, got {status}\n{}",
            self.combined_output()
        );
    }

    pub fn wait_for_exit(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;

        loop {
            if let Some(status) = self.child.try_wait().expect("child status should be readable") {
                return status;
            }

            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!("timed out waiting for rginx to exit\n{}", self.combined_output());
            }

            thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn assert_running(&mut self) {
        if let Some(status) = self.child.try_wait().expect("child status should be readable") {
            panic!("rginx exited unexpectedly with status {status}\n{}", self.combined_output());
        }
    }

    pub fn combined_output(&self) -> String {
        let stdout = read_optional_log(&self.stdout_path);
        let stderr = read_optional_log(&self.stderr_path);
        format!("stdout:\n{stdout}\nstderr:\n{stderr}")
    }
}

impl Drop for ServerHarness {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }

        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}
