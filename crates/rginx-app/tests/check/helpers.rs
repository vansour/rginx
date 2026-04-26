use super::*;

pub(crate) fn run_rginx(args: impl IntoIterator<Item = impl AsRef<str>>) -> Output {
    let mut command = Command::new(binary_path());
    for arg in args {
        command.arg(arg.as_ref());
    }

    command.output().expect("rginx should run")
}

pub(crate) fn write_return_config(path: &Path, listen_addr: SocketAddr, body: &str) {
    fs::write(path, return_config(listen_addr, body)).expect("config file should be written");
}

pub(crate) fn return_config(listen_addr: SocketAddr, body: &str) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some({:?}),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        body
    )
}

pub(crate) fn temp_dir(prefix: &str) -> PathBuf {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time should be after unix epoch")
        .as_nanos();
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!("{prefix}-{unique}-{id}"))
}

pub(crate) fn binary_path() -> PathBuf {
    env::var_os("CARGO_BIN_EXE_rginx")
        .map(PathBuf::from)
        .expect("cargo should expose the rginx test binary path")
}

pub(crate) fn render_output(output: &Output) -> String {
    format!(
        "status={}; stdout={:?}; stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub(crate) type TestCertifiedKey = CertifiedKey<KeyPair>;

pub(crate) fn generate_cert(hostname: &str) -> TestCertifiedKey {
    rcgen::generate_simple_self_signed(vec![hostname.to_string()])
        .expect("self-signed certificate should generate")
}

pub(crate) fn generate_cert_signed_by_ca(hostname: &str) -> TestCertifiedKey {
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params.distinguished_name.push(DnType::CommonName, "check test ca");
    let ca_key = KeyPair::generate().expect("CA key should generate");
    let ca_cert = ca_params.self_signed(&ca_key).expect("CA cert should generate");
    let _ca = CertifiedKey { cert: ca_cert, signing_key: ca_key };
    let ca_issuer = rcgen::Issuer::from_params(&ca_params, &_ca.signing_key);

    let mut leaf_params =
        CertificateParams::new(vec![hostname.to_string()]).expect("leaf params should build");
    leaf_params.distinguished_name.push(DnType::CommonName, hostname);
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf_key = KeyPair::generate().expect("leaf key should generate");
    let cert = leaf_params.signed_by(&leaf_key, &ca_issuer).expect("leaf cert should be signed");
    CertifiedKey { cert, signing_key: leaf_key }
}
