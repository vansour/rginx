use std::env;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[test]
fn proxies_http_upgrade_streams_end_to_end() {
    let upstream_listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("upstream listener should bind");
    let upstream_addr =
        upstream_listener.local_addr().expect("upstream listener addr should be available");
    let upstream_task = thread::spawn(move || {
        let (mut stream, _) = upstream_listener.accept().expect("upstream should accept a client");
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("upstream read timeout should be configurable");
        stream
            .set_write_timeout(Some(Duration::from_secs(2)))
            .expect("upstream write timeout should be configurable");

        let request = read_http_head(&mut stream);
        let request_lower = request.to_ascii_lowercase();
        assert!(
            request.starts_with("GET /ws HTTP/1.1\r\n"),
            "unexpected upstream request line: {request:?}"
        );
        assert!(
            request_lower.contains("\r\nconnection: upgrade\r\n"),
            "upgrade connection header should be preserved: {request:?}"
        );
        assert!(
            request_lower.contains("\r\nupgrade: websocket\r\n"),
            "upgrade protocol header should be preserved: {request:?}"
        );

        stream
            .write_all(
                b"HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n",
            )
            .expect("upstream should write switching protocols response");
        stream.flush().expect("upstream response should flush");

        let mut payload = [0u8; 4];
        stream.read_exact(&mut payload).expect("upstream should read tunneled payload");
        assert_eq!(&payload, b"ping");

        stream.write_all(b"pong").expect("upstream should write tunneled response payload");
        stream.flush().expect("upstream tunneled response should flush");
    });

    let listen_addr = reserve_loopback_addr();
    let mut server =
        TestServer::spawn(listen_addr, upgrade_proxy_config(listen_addr, upstream_addr));
    let mut client = wait_for_listener(listen_addr, Duration::from_secs(5));
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("client read timeout should be configurable");
    client
        .set_write_timeout(Some(Duration::from_secs(2)))
        .expect("client write timeout should be configurable");

    write!(
        client,
        "GET /ws HTTP/1.1\r\nHost: app.example.com\r\nConnection: keep-alive, Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGVzdC1rZXk=\r\n\r\n"
    )
    .expect("client should write upgrade request");
    client.flush().expect("upgrade request should flush");

    let response = read_http_head(&mut client);
    let response_lower = response.to_ascii_lowercase();
    assert!(response.starts_with("HTTP/1.1 101"), "unexpected upgrade response line: {response:?}");
    assert!(
        response_lower.contains("\r\nconnection: upgrade\r\n"),
        "upgrade response should preserve connection header: {response:?}"
    );
    assert!(
        response_lower.contains("\r\nupgrade: websocket\r\n"),
        "upgrade response should preserve protocol header: {response:?}"
    );

    client.write_all(b"ping").expect("client should write tunneled payload");
    client.flush().expect("client tunneled payload should flush");

    let mut payload = [0u8; 4];
    client.read_exact(&mut payload).expect("client should read tunneled response payload");
    assert_eq!(&payload, b"pong");

    drop(client);
    server.shutdown_and_wait(Duration::from_secs(5));
    upstream_task.join().expect("upstream thread should complete");
}

struct TestServer {
    child: Child,
    temp_dir: PathBuf,
}

impl TestServer {
    fn spawn(listen_addr: SocketAddr, config: String) -> Self {
        let temp_dir = temp_dir("rginx-upgrade-test");
        fs::create_dir_all(&temp_dir).expect("temp test dir should be created");
        let config_path = temp_dir.join("rginx.ron");
        fs::write(&config_path, config).expect("upgrade test config should be written");

        let child = Command::new(binary_path())
            .arg("--config")
            .arg(&config_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("rginx should start");

        let _ = listen_addr;
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

fn wait_for_listener(listen_addr: SocketAddr, timeout: Duration) -> TcpStream {
    let deadline = Instant::now() + timeout;
    let mut last_error = String::new();

    while Instant::now() < deadline {
        match TcpStream::connect_timeout(&listen_addr, Duration::from_millis(200)) {
            Ok(stream) => return stream,
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

        if buffer.windows(4).any(|window| window == b"\r\n\r\n") {
            let head_end = buffer
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .expect("header terminator should exist")
                + 4;
            return String::from_utf8(buffer[..head_end].to_vec())
                .expect("HTTP head should be valid UTF-8");
        }
    }
}

fn upgrade_proxy_config(listen_addr: SocketAddr, upstream_addr: SocketAddr) -> String {
    format!(
        "Config(\n    runtime: RuntimeConfig(\n        shutdown_timeout_secs: 2,\n    ),\n    server: ServerConfig(\n        listen: {:?},\n    ),\n    upstreams: [\n        UpstreamConfig(\n            name: \"backend\",\n            peers: [\n                UpstreamPeerConfig(\n                    url: {:?},\n                ),\n            ],\n        ),\n    ],\n    locations: [\n        LocationConfig(\n            matcher: Prefix(\"/ws\"),\n            handler: Proxy(\n                upstream: \"backend\",\n            ),\n        ),\n    ],\n)\n",
        listen_addr.to_string(),
        format!("http://{upstream_addr}")
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
