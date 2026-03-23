use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn static_responses_generate_and_preserve_request_id_headers() {
    let listen_addr = reserve_loopback_addr();
    let mut server =
        TestServer::spawn("rginx-phase1-static", |_| static_config(listen_addr, "ok\n"));
    wait_for_listener(listen_addr, Duration::from_secs(5));

    let head_response = send_http_request(
        listen_addr,
        &format!("HEAD / HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n"),
    )
    .expect("HEAD request should succeed");
    assert_eq!(head_response.status, 200);
    assert_eq!(head_response.body, b"");
    assert_eq!(head_response.header("content-length"), Some("3"));
    assert_generated_request_id(head_response.header("x-request-id"));

    let get_response = send_http_request(
        listen_addr,
        &format!(
            "GET / HTTP/1.1\r\nHost: {listen_addr}\r\nX-Request-ID: client-static-42\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("GET request should succeed");
    assert_eq!(get_response.status, 200);
    assert_eq!(get_response.body, b"ok\n");
    assert_eq!(get_response.header("x-request-id"), Some("client-static-42"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

#[test]
fn proxy_preserves_request_id_end_to_end() {
    let upstream_listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("test upstream listener should bind");
    let upstream_addr =
        upstream_listener.local_addr().expect("upstream listener addr should be available");
    let upstream_task = thread::spawn(move || {
        let (mut stream, _) = upstream_listener.accept().expect("upstream should accept");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("upstream read timeout should be configurable");
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("upstream write timeout should be configurable");

        let request = read_http_head(&mut stream);
        assert!(
            request.to_ascii_lowercase().contains("x-request-id: client-proxy-42\r\n"),
            "proxied request should preserve the incoming request id, got {request:?}"
        );

        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\ncontent-type: text/plain; charset=utf-8\r\ncontent-length: 11\r\nconnection: close\r\n\r\nbackend ok\n",
            )
            .expect("upstream should write a response");
        stream.flush().expect("upstream response should flush");
    });

    let listen_addr = reserve_loopback_addr();
    let mut server =
        TestServer::spawn("rginx-phase1-proxy", |_| proxy_config(listen_addr, upstream_addr));
    wait_for_listener(listen_addr, Duration::from_secs(5));

    let response = send_http_request(
        listen_addr,
        &format!(
            "GET /api/demo HTTP/1.1\r\nHost: {listen_addr}\r\nX-Request-ID: client-proxy-42\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("proxy request should succeed");
    assert_eq!(response.status, 200);
    assert_eq!(response.body, b"backend ok\n");
    assert_eq!(response.header("x-request-id"), Some("client-proxy-42"));

    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.join().expect("upstream thread should complete");
}

#[test]
fn file_routes_support_head_and_range_requests() {
    let listen_addr = reserve_loopback_addr();
    let mut server = TestServer::spawn("rginx-phase1-file", |temp_dir| {
        let root = temp_dir.join("public");
        fs::create_dir_all(&root).expect("file root should be created");
        fs::write(root.join("hello.txt"), b"0123456789abcdef")
            .expect("test file should be written");
        file_config(listen_addr, &root)
    });
    wait_for_listener(listen_addr, Duration::from_secs(5));

    let head_response = send_http_request(
        listen_addr,
        &format!("HEAD /hello.txt HTTP/1.1\r\nHost: {listen_addr}\r\nConnection: close\r\n\r\n"),
    )
    .expect("HEAD file request should succeed");
    assert_eq!(head_response.status, 200);
    assert_eq!(head_response.body, b"");
    assert_eq!(head_response.header("accept-ranges"), Some("bytes"));
    assert_eq!(head_response.header("content-length"), Some("16"));
    assert_generated_request_id(head_response.header("x-request-id"));

    let range_response = send_http_request(
        listen_addr,
        &format!(
            "GET /hello.txt HTTP/1.1\r\nHost: {listen_addr}\r\nRange: bytes=2-5\r\nConnection: close\r\n\r\n"
        ),
    )
    .expect("range request should succeed");
    assert_eq!(range_response.status, 206);
    assert_eq!(range_response.body, b"2345");
    assert_eq!(range_response.header("accept-ranges"), Some("bytes"));
    assert_eq!(range_response.header("content-length"), Some("4"));
    assert_eq!(range_response.header("content-range"), Some("bytes 2-5/16"));

    server.shutdown_and_wait(Duration::from_secs(5));
}

struct TestServer {
    child: Child,
    temp_dir: PathBuf,
}

impl TestServer {
    fn spawn(prefix: &str, build_config: impl FnOnce(&Path) -> String) -> Self {
        let temp_dir = temp_dir(prefix);
        fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
        let config_path = temp_dir.join("rginx.ron");
        fs::write(&config_path, build_config(&temp_dir)).expect("config should be written");

        let child = Command::new(binary_path())
            .arg("--config")
            .arg(&config_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("rginx should start");

        Self { child, temp_dir }
    }

    fn shutdown_and_wait(&mut self, timeout: Duration) {
        self.child.kill().expect("rginx should accept a kill signal");
        let status = self.wait_for_exit(timeout);
        assert!(
            !status.success() || status.code() == Some(0),
            "rginx should exit after the test, got {status}"
        );
    }

    fn wait_for_exit(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;

        loop {
            if let Some(status) = self.child.try_wait().expect("child status should be readable") {
                return status;
            }

            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!("timed out waiting for rginx to exit");
            }

            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }

        let _ = fs::remove_dir_all(&self.temp_dir);
    }
}

#[derive(Debug)]
struct ParsedResponse {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl ParsedResponse {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }
}

fn send_http_request(listen_addr: SocketAddr, request: &str) -> Result<ParsedResponse, String> {
    let mut stream = TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200))
        .map_err(|error| format!("failed to connect to {listen_addr}: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| format!("failed to set read timeout: {error}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(500)))
        .map_err(|error| format!("failed to set write timeout: {error}"))?;
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("failed to write request: {error}"))?;
    stream.flush().map_err(|error| format!("failed to flush request: {error}"))?;

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| format!("failed to read response: {error}"))?;
    parse_http_response(&response)
}

fn parse_http_response(bytes: &[u8]) -> Result<ParsedResponse, String> {
    let head_end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| format!("malformed response: {bytes:?}"))?;
    let head = String::from_utf8(bytes[..head_end].to_vec())
        .map_err(|error| format!("response head should be valid UTF-8: {error}"))?;
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| format!("missing status line: {head:?}"))?
        .parse::<u16>()
        .map_err(|error| format!("invalid status code: {error}"))?;

    let mut headers = HashMap::new();
    for line in head.lines().skip(1) {
        let Some((name, value)) = line.split_once(':') else {
            return Err(format!("malformed header line: {line:?}"));
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }

    Ok(ParsedResponse { status, headers, body: bytes[head_end + 4..].to_vec() })
}

fn wait_for_listener(listen_addr: SocketAddr, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let mut last_error = String::new();

    while Instant::now() < deadline {
        match TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200)) {
            Ok(_) => return,
            Err(error) => last_error = error.to_string(),
        }

        thread::sleep(Duration::from_millis(50));
    }

    panic!("timed out waiting for rginx to listen on {listen_addr}: {last_error}");
}

fn read_http_head(stream: &mut TcpStream) -> String {
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

fn assert_generated_request_id(value: Option<&str>) {
    let value = value.expect("response should include x-request-id");
    assert_eq!(value.len(), "rginx-0000000000000000".len());
    assert!(value.starts_with("rginx-"), "generated request id should use the rginx- prefix");
    assert!(
        value["rginx-".len()..].chars().all(|ch| ch.is_ascii_hexdigit()),
        "generated request id should end with lowercase hex digits, got {value:?}"
    );
}

fn static_config(listen_addr: SocketAddr, body: &str) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Exact(\"/\"),\n            handler: Static(\n                body: {:?},\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        body
    )
}

fn proxy_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n        ),\n    ],\n    locations: [\n        LocationConfig(\n            matcher: Prefix(\"/api\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}")
    )
}

fn file_config(listen_addr: SocketAddr, root: &Path) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [],\n    locations: [\n        LocationConfig(\n            matcher: Prefix(\"/\"),\n            handler: File(\n                root: {:?},\n                index: None,\n                try_files: Some([\"$uri\"]),\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        root.display().to_string()
    )
}

fn reserve_loopback_addr() -> SocketAddr {
    let listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("ephemeral loopback listener should bind");
    let addr = listener.local_addr().expect("listener addr should be available");
    drop(listener);
    addr
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
