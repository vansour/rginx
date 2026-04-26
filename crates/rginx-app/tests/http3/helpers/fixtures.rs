use super::*;

pub(crate) fn generate_cert(hostname: &str) -> rcgen::CertifiedKey<rcgen::KeyPair> {
    rcgen::generate_simple_self_signed(vec![hostname.to_string()])
        .expect("self-signed certificate should generate")
}

pub(crate) struct Http3MtlsFixture {
    pub(crate) _dir: PathBuf,
    pub(crate) ca_cert_pem: String,
    pub(crate) server_cert_pem: String,
    pub(crate) server_key_pem: String,
    pub(crate) client_cert_path: PathBuf,
    pub(crate) client_key_path: PathBuf,
}

impl Http3MtlsFixture {
    pub(crate) fn new(prefix: &str) -> Self {
        let dir = temp_dir(prefix);
        fs::create_dir_all(&dir).expect("fixture temp dir should be created");

        let ca = generate_ca_cert("rginx h3 mtls ca");
        let server =
            generate_cert_signed_by_ca("localhost", &ca, ExtendedKeyUsagePurpose::ServerAuth);
        let client = generate_cert_signed_by_ca(
            "client.example.com",
            &ca,
            ExtendedKeyUsagePurpose::ClientAuth,
        );

        let client_cert_path = dir.join("client.crt");
        let client_key_path = dir.join("client.key");
        fs::write(&client_cert_path, client.cert.pem()).expect("client cert should be written");
        fs::write(&client_key_path, client.signing_key.serialize_pem())
            .expect("client key should be written");

        Self {
            _dir: dir,
            ca_cert_pem: ca.cert.pem(),
            server_cert_pem: server.cert.pem(),
            server_key_pem: server.signing_key.serialize_pem(),
            client_cert_path,
            client_key_path,
        }
    }
}

pub(crate) fn temp_dir(prefix: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    PathBuf::from(format!("{}/{}-{}-{}", std::env::temp_dir().display(), prefix, unique, id))
}

pub(crate) struct TestCertifiedKey {
    pub(crate) cert: rcgen::Certificate,
    pub(crate) signing_key: KeyPair,
    pub(crate) params: CertificateParams,
}

impl TestCertifiedKey {
    pub(crate) fn issuer(&self) -> Issuer<'_, &KeyPair> {
        Issuer::from_params(&self.params, &self.signing_key)
    }
}

pub(crate) fn generate_ca_cert(common_name: &str) -> TestCertifiedKey {
    let mut params = CertificateParams::default();
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name.push(DnType::CommonName, common_name);
    let signing_key = KeyPair::generate().expect("CA keypair should generate");
    let cert = params.self_signed(&signing_key).expect("CA cert should generate");
    TestCertifiedKey { cert, signing_key, params }
}

pub(crate) fn generate_cert_signed_by_ca(
    dns_name: &str,
    issuer: &TestCertifiedKey,
    usage: ExtendedKeyUsagePurpose,
) -> TestCertifiedKey {
    let mut params =
        CertificateParams::new(vec![dns_name.to_string()]).expect("leaf params should build");
    params.distinguished_name.push(DnType::CommonName, dns_name);
    params.extended_key_usages = vec![usage];
    let signing_key = KeyPair::generate().expect("leaf keypair should generate");
    let cert =
        params.signed_by(&signing_key, &issuer.issuer()).expect("leaf cert should be signed");
    TestCertifiedKey { cert, signing_key, params }
}

pub(crate) fn decode_gzip(bytes: &[u8]) -> Vec<u8> {
    let mut decoder = GzDecoder::new(bytes);
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).expect("gzip body should decode");
    decoded
}

pub(crate) fn body_text(response: &Http3Response) -> String {
    String::from_utf8(response.body.clone()).expect("response body should be valid UTF-8")
}

impl Http3Response {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }
}

pub(crate) fn wait_for_admin_socket(path: &Path, timeout: Duration) {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if path.exists() && UnixStream::connect(path).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }

    panic!("timed out waiting for admin socket {}", path.display());
}

pub(crate) async fn wait_for_http3_text_response(
    listen_addr: SocketAddr,
    server_name: &str,
    path: &str,
    client_identity: Option<(&Path, &Path)>,
    expected_status: u16,
    expected_body: &str,
    cert_pem: &str,
    timeout: Duration,
) {
    let deadline = std::time::Instant::now() + timeout;
    let mut last_error = String::new();

    while std::time::Instant::now() < deadline {
        match http3_get_with_client_identity(
            listen_addr,
            server_name,
            path,
            client_identity,
            cert_pem,
        )
        .await
        {
            Ok(response)
                if response.status.as_u16() == expected_status
                    && body_text(&response) == expected_body =>
            {
                return;
            }
            Ok(response) => {
                last_error = format!(
                    "unexpected response: status={} body={:?}",
                    response.status.as_u16(),
                    body_text(&response)
                );
            }
            Err(error) => last_error = error,
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    panic!(
        "timed out waiting for expected HTTP/3 response on {listen_addr}{path}; last error: {last_error}"
    );
}

pub(crate) fn parse_flat_u64(output: &str, key: &str) -> u64 {
    output
        .split_whitespace()
        .find_map(|field| field.strip_prefix(&format!("{key}=")))
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0)
}

pub(crate) fn run_rginx(args: impl IntoIterator<Item = impl AsRef<str>>) -> std::process::Output {
    let mut command = std::process::Command::new(binary_path());
    for arg in args {
        command.arg(arg.as_ref());
    }
    command.output().expect("rginx command should run")
}

pub(crate) fn binary_path() -> std::path::PathBuf {
    std::env::var_os("CARGO_BIN_EXE_rginx")
        .map(std::path::PathBuf::from)
        .expect("cargo should expose the rginx test binary path")
}

pub(crate) fn render_output(output: &std::process::Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub(crate) fn read_http_head_from_stream(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 256];

    loop {
        let read = stream.read(&mut chunk).expect("HTTP head should be readable");
        assert!(read > 0, "stream closed before HTTP head was complete");
        buffer.extend_from_slice(&chunk[..read]);

        if let Some(head_end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            return String::from_utf8(buffer[..head_end + 4].to_vec())
                .expect("HTTP head should be valid UTF-8");
        }
    }
}

pub(crate) fn write_chunked_payload(stream: &mut std::net::TcpStream, chunk: &[u8]) {
    write!(stream, "{:x}\r\n", chunk.len()).expect("chunk header should write");
    stream.write_all(chunk).expect("chunk payload should write");
    stream.write_all(b"\r\n").expect("chunk terminator should write");
    stream.flush().expect("chunk should flush");
}
