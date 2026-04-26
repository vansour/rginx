use super::*;

pub(super) fn binary_path() -> std::path::PathBuf {
    std::env::var_os("CARGO_BIN_EXE_rginx")
        .map(std::path::PathBuf::from)
        .expect("cargo should expose the rginx test binary path")
}

pub(super) fn render_output(output: &Output) -> String {
    format!(
        "status={}; stdout={:?}; stderr={:?}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

pub(super) type TestCertifiedKey = CertifiedKey<KeyPair>;

pub(super) fn generate_cert(hostname: &str) -> TestCertifiedKey {
    rcgen::generate_simple_self_signed(vec![hostname.to_string()])
        .expect("self-signed certificate should generate")
}

pub(super) fn tls_return_config(
    listen_addr: SocketAddr,
    cert_path: &Path,
    key_path: &Path,
) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n        server_names: [\"localhost\"],\n        tls: Some(ServerTlsConfig(\n            cert_path: {:?},\n            key_path: {:?},\n        )),\n    ),\n    upstreams: [],\n    locations: [\n{ready_route}        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Return(\n                status: 200,\n                location: \"\",\n                body: Some(\"tls reload\\n\"),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        cert_path.display().to_string(),
        key_path.display().to_string(),
        ready_route = READY_ROUTE_CONFIG,
    )
}

pub(super) fn run_cli_command<'a>(
    config_path: &Path,
    args: impl IntoIterator<Item = &'a str>,
) -> Output {
    let mut command = Command::new(binary_path());
    command.arg("--config").arg(config_path);
    for arg in args {
        command.arg(arg);
    }
    command.output().expect("rginx command should run")
}

pub(super) fn wait_for_status_output(
    config_path: &Path,
    predicate: impl Fn(&str) -> bool,
    timeout: Duration,
) -> String {
    let deadline = Instant::now() + timeout;
    let mut last_output = String::new();

    while Instant::now() < deadline {
        let output = run_cli_command(config_path, ["status"]);
        let rendered = render_output(&output);

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
            if predicate(&stdout) {
                return stdout;
            }
        }

        last_output = rendered;
        std::thread::sleep(Duration::from_millis(50));
    }

    panic!(
        "timed out waiting for rginx status on {} to satisfy the expected condition; last output: {}",
        config_path.display(),
        last_output
    );
}

pub(super) fn test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
