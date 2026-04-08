#![allow(dead_code)]

use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, SignatureScheme, StreamOwned};

pub const READY_ROUTE_CONFIG: &str = "        LocationConfig(\n            matcher: Exact(\"/-/ready\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"ready\\n\"),\n            ),\n        ),\n";

const READY_PATH: &str = "/-/ready";
const READY_BODY: &str = "ready\n";
const DEFAULT_TLS_SERVER_NAME: &str = "localhost";

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

pub fn reserve_loopback_addr() -> SocketAddr {
    let listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("ephemeral loopback listener should bind");
    let addr = listener.local_addr().expect("listener addr should be available");
    drop(listener);
    addr
}

pub fn read_http_head(stream: &mut TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 256];

    loop {
        let read = stream.read(&mut chunk).expect("HTTP head should be readable");
        assert!(read > 0, "stream closed before the HTTP head was complete");
        buffer.extend_from_slice(&chunk[..read]);

        if let Some(head_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            return String::from_utf8(buffer[..head_end + 4].to_vec())
                .expect("HTTP head should be valid UTF-8");
        }
    }
}

pub fn apply_tls_placeholders(config: String, cert_path: &Path, key_path: &Path) -> String {
    config
        .replace("__CERT_PATH__", &cert_path.display().to_string())
        .replace("__KEY_PATH__", &key_path.display().to_string())
}

fn fetch_http_text_response(
    listen_addr: SocketAddr,
    host: &str,
    path: &str,
) -> Result<(u16, String), String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;

    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
        .map_err(|error| format!("failed to write request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush request: {error}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("failed to read response: {error}"))?;
    parse_text_response(&response)
}

fn fetch_https_text_response(
    listen_addr: SocketAddr,
    host: &str,
    path: &str,
    server_name: &str,
) -> Result<(u16, String), String> {
    let tcp = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    tcp.set_read_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    tcp.set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;

    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(InsecureServerCertVerifier::new()))
        .with_no_client_auth();
    let server_name = ServerName::try_from(server_name.to_string())
        .map_err(|error| format!("invalid TLS server name `{server_name}`: {error}"))?;
    let connection = ClientConnection::new(Arc::new(config), server_name)
        .map_err(|error| format!("failed to build TLS client: {error}"))?;
    let mut stream = StreamOwned::new(connection, tcp);

    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n")
        .map_err(|error| format!("failed to write HTTPS request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush HTTPS request: {error}"))?;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|error| format!("failed to read HTTPS response: {error}"))?;
    parse_text_response(&response)
}

fn parse_text_response(response: &str) -> Result<(u16, String), String> {
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| format!("malformed response: {response:?}"))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| format!("missing status line: {head:?}"))?
        .parse::<u16>()
        .map_err(|error| format!("invalid status code: {error}"))?;

    Ok((status, body.to_string()))
}

fn read_optional_log(path: &Path) -> String {
    match fs::read_to_string(path) {
        Ok(contents) if contents.is_empty() => "<empty>".to_string(),
        Ok(contents) => contents,
        Err(error) => format!("<unavailable: {error}>"),
    }
}

fn temp_dir(prefix: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!("{prefix}-{unique}-{id}"))
}

fn binary_path() -> PathBuf {
    env::var_os("CARGO_BIN_EXE_rginx")
        .map(PathBuf::from)
        .expect("cargo should expose the rginx test binary path")
}

#[derive(Debug)]
struct InsecureServerCertVerifier {
    supported_schemes: Vec<SignatureScheme>,
}

impl InsecureServerCertVerifier {
    fn new() -> Self {
        Self {
            supported_schemes: rustls::crypto::aws_lc_rs::default_provider()
                .signature_verification_algorithms
                .supported_schemes(),
        }
    }
}

impl ServerCertVerifier for InsecureServerCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.supported_schemes.clone()
    }
}
