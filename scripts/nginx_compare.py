#!/usr/bin/env python3
from __future__ import annotations

import argparse
import base64
import concurrent.futures
import contextlib
import dataclasses
import http.server
import json
import os
import pathlib
import re
import shutil
import signal
import socket
import ssl
import statistics
import subprocess
import sys
import tempfile
import textwrap
import threading
import time


NGINX_REPO_URL = "https://github.com/nginx/nginx"


@dataclasses.dataclass(frozen=True)
class BenchmarkResult:
    server: str
    scenario: str
    tool: str
    complete_requests: int
    failed_requests: int
    requests_per_sec: float
    time_per_request_ms: float
    transfer_rate_kb_sec: float | None


@dataclasses.dataclass(frozen=True)
class UnsupportedScenario:
    server: str
    scenario: str
    reason: str


@dataclasses.dataclass(frozen=True)
class ReloadResult:
    server: str
    scenario: str
    reload_apply_ms: float


def run(
    command: list[str],
    *,
    cwd: pathlib.Path | None = None,
    env: dict[str, str] | None = None,
    capture_output: bool = False,
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        command,
        cwd=str(cwd) if cwd is not None else None,
        env=env,
        check=False,
        text=True,
        capture_output=capture_output,
    )
    if completed.returncode != 0:
        stdout = completed.stdout or ""
        stderr = completed.stderr or ""
        raise RuntimeError(
            f"command failed ({completed.returncode}): {' '.join(command)}\nstdout:\n{stdout}\nstderr:\n{stderr}"
        )
    return completed


def reserve_port() -> int:
    with contextlib.closing(socket.socket(socket.AF_INET, socket.SOCK_STREAM)) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class UpstreamHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:
        if self.path == "/-/ready":
            body = b"ready\n"
        else:
            body = b"ok\n"

        self.send_response(200)
        self.send_header("Content-Type", "text/plain; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt: str, *args: object) -> None:
        return


def start_upstream_server() -> tuple[http.server.ThreadingHTTPServer, threading.Thread, int]:
    port = reserve_port()
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), UpstreamHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread, port


def fetch_text_response(port: int, path: str, *, tls_enabled: bool) -> tuple[int, str]:
    with contextlib.closing(socket.create_connection(("127.0.0.1", port), timeout=1.0)) as sock:
        conn: socket.socket | ssl.SSLSocket
        conn = sock
        if tls_enabled:
            context = ssl.create_default_context()
            context.check_hostname = False
            context.verify_mode = ssl.CERT_NONE
            conn = context.wrap_socket(sock, server_hostname="localhost")

        conn.sendall(
            (
                f"GET {path} HTTP/1.1\r\n"
                f"Host: localhost\r\n"
                f"Connection: close\r\n\r\n"
            ).encode("ascii")
        )
        chunks: list[bytes] = []
        while True:
            chunk = conn.recv(4096)
            if not chunk:
                break
            chunks.append(chunk)

    raw = b"".join(chunks)
    head, _, body = raw.partition(b"\r\n\r\n")
    status_line = head.split(b"\r\n", 1)[0]
    match = re.match(rb"HTTP/\d\.\d\s+(\d+)", status_line)
    if match is None:
        raise RuntimeError(f"invalid HTTP response from 127.0.0.1:{port}: {raw[:200]!r}")
    return int(match.group(1)), body.decode("utf-8", errors="replace")


def wait_for_ready(port: int, *, tls_enabled: bool, timeout_secs: float = 20.0) -> None:
    deadline = time.time() + timeout_secs
    while time.time() < deadline:
        try:
            status, body = fetch_text_response(port, "/-/ready", tls_enabled=tls_enabled)
        except OSError:
            status, body = 0, ""
        except ssl.SSLError:
            status, body = 0, ""
        if status == 200 and body == "ready\n":
            return
        time.sleep(0.1)
    raise RuntimeError(f"timed out waiting for 127.0.0.1:{port} to become ready")


def generate_self_signed_cert(cert_path: pathlib.Path, key_path: pathlib.Path) -> None:
    run(
        [
            "openssl",
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-days",
            "3",
            "-nodes",
            "-keyout",
            str(key_path),
            "-out",
            str(cert_path),
            "-subj",
            "/CN=localhost",
            "-addext",
            "subjectAltName=DNS:localhost,IP:127.0.0.1",
        ]
    )


def run_curl_request(
    *,
    url: str,
    flags: list[str],
    headers: list[str],
    body: bytes | None,
    timeout_secs: float,
) -> float:
    started = time.perf_counter()
    command = [
        "curl",
        "--silent",
        "--show-error",
        "--fail",
        "--output",
        "/dev/null",
        "--max-time",
        str(timeout_secs),
        *flags,
    ]
    for header in headers:
        command.extend(["--header", header])
    command.append(url)
    subprocess.run(command, input=body, check=True)
    return time.perf_counter() - started


def rginx_return_config(port: int) -> str:
    return textwrap.dedent(
        f"""\
        Config(
            runtime: RuntimeConfig(
                shutdown_timeout_secs: 5,
                worker_threads: Some(1),
                accept_workers: Some(1),
            ),
            server: ServerConfig(
                listen: "127.0.0.1:{port}",
                keep_alive: Some(true),
            ),
            upstreams: [],
            locations: [
                LocationConfig(
                    matcher: Exact("/-/ready"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ready\\n"),
                    ),
                ),
                LocationConfig(
                    matcher: Exact("/"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ok\\n"),
                    ),
                ),
            ],
            servers: [],
        )
        """
    )


def rginx_proxy_config(port: int, upstream_port: int) -> str:
    return textwrap.dedent(
        f"""\
        Config(
            runtime: RuntimeConfig(
                shutdown_timeout_secs: 5,
                worker_threads: Some(1),
                accept_workers: Some(1),
            ),
            server: ServerConfig(
                listen: "127.0.0.1:{port}",
                keep_alive: Some(true),
            ),
            upstreams: [
                UpstreamConfig(
                    name: "backend",
                    peers: [UpstreamPeerConfig(url: "http://127.0.0.1:{upstream_port}")],
                    protocol: Http1,
                    load_balance: RoundRobin,
                ),
            ],
            locations: [
                LocationConfig(
                    matcher: Exact("/-/ready"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ready\\n"),
                    ),
                ),
                LocationConfig(
                    matcher: Prefix("/"),
                    handler: Proxy(upstream: "backend"),
                ),
            ],
            servers: [],
        )
        """
    )


def rginx_tls_return_config(port: int, cert_path: pathlib.Path, key_path: pathlib.Path) -> str:
    return textwrap.dedent(
        f"""\
        Config(
            runtime: RuntimeConfig(
                shutdown_timeout_secs: 5,
                worker_threads: Some(1),
                accept_workers: Some(1),
            ),
            server: ServerConfig(
                listen: "127.0.0.1:{port}",
                keep_alive: Some(true),
                tls: Some(ServerTlsConfig(
                    cert_path: "{cert_path}",
                    key_path: "{key_path}",
                )),
            ),
            upstreams: [],
            locations: [
                LocationConfig(
                    matcher: Exact("/-/ready"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ready\\n"),
                    ),
                ),
                LocationConfig(
                    matcher: Exact("/"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ok\\n"),
                    ),
                ),
            ],
            servers: [],
        )
        """
    )


def rginx_grpc_proxy_config(
    port: int,
    cert_path: pathlib.Path,
    key_path: pathlib.Path,
    backend_port: int,
) -> str:
    return textwrap.dedent(
        f"""\
        Config(
            runtime: RuntimeConfig(
                shutdown_timeout_secs: 5,
                worker_threads: Some(1),
                accept_workers: Some(1),
            ),
            server: ServerConfig(
                listen: "127.0.0.1:{port}",
                keep_alive: Some(true),
                tls: Some(ServerTlsConfig(
                    cert_path: "{cert_path}",
                    key_path: "{key_path}",
                )),
            ),
            upstreams: [
                UpstreamConfig(
                    name: "grpc-backend",
                    peers: [UpstreamPeerConfig(url: "https://localhost:{backend_port}")],
                    tls: Some(Insecure),
                    protocol: Http2,
                ),
            ],
            locations: [
                LocationConfig(
                    matcher: Exact("/-/ready"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ready\\n"),
                    ),
                ),
                LocationConfig(
                    matcher: Prefix("/"),
                    handler: Proxy(upstream: "grpc-backend"),
                ),
            ],
            servers: [],
        )
        """
    )


def rginx_reload_config(port: int, body: str) -> str:
    return textwrap.dedent(
        f"""\
        Config(
            runtime: RuntimeConfig(
                shutdown_timeout_secs: 5,
                worker_threads: Some(1),
                accept_workers: Some(1),
            ),
            server: ServerConfig(
                listen: "127.0.0.1:{port}",
                keep_alive: Some(true),
            ),
            upstreams: [],
            locations: [
                LocationConfig(
                    matcher: Exact("/-/ready"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("ready\\n"),
                    ),
                ),
                LocationConfig(
                    matcher: Exact("/"),
                    handler: Return(
                        status: 200,
                        location: "",
                        body: Some("{body}"),
                    ),
                ),
            ],
            servers: [],
        )
        """
    )


def nginx_return_config(port: int) -> str:
    return textwrap.dedent(
        f"""\
        worker_processes 1;
        error_log logs/error.log warn;
        pid logs/nginx.pid;

        events {{
            worker_connections 4096;
        }}

        http {{
            access_log off;
            keepalive_timeout 65;

            server {{
                listen 127.0.0.1:{port};

                location = /-/ready {{
                    default_type text/plain;
                    return 200 "ready\\n";
                }}

                location = / {{
                    default_type text/plain;
                    return 200 "ok\\n";
                }}
            }}
        }}
        """
    )


def nginx_tls_return_config(port: int, cert_path: pathlib.Path, key_path: pathlib.Path) -> str:
    return textwrap.dedent(
        f"""\
        worker_processes 1;
        error_log logs/error.log warn;
        pid logs/nginx.pid;

        events {{
            worker_connections 4096;
        }}

        http {{
            access_log off;
            keepalive_timeout 65;

            server {{
                listen 127.0.0.1:{port} ssl http2;
                ssl_certificate {cert_path};
                ssl_certificate_key {key_path};

                location = /-/ready {{
                    default_type text/plain;
                    return 200 "ready\\n";
                }}

                location = / {{
                    default_type text/plain;
                    return 200 "ok\\n";
                }}
            }}
        }}
        """
    )


def nginx_grpc_proxy_config(
    port: int,
    cert_path: pathlib.Path,
    key_path: pathlib.Path,
    backend_port: int,
) -> str:
    return textwrap.dedent(
        f"""\
        worker_processes 1;
        error_log logs/error.log warn;
        pid logs/nginx.pid;

        events {{
            worker_connections 4096;
        }}

        http {{
            access_log off;
            keepalive_timeout 65;

            server {{
                listen 127.0.0.1:{port} ssl http2;
                ssl_certificate {cert_path};
                ssl_certificate_key {key_path};

                location = /-/ready {{
                    default_type text/plain;
                    return 200 "ready\\n";
                }}

                location = /bench.Bench/Ping {{
                    grpc_ssl_name localhost;
                    grpc_ssl_verify off;
                    grpc_pass grpcs://localhost:{backend_port};
                }}
            }}
        }}
        """
    )


def nginx_reload_config(port: int, body: str) -> str:
    return textwrap.dedent(
        f"""\
        worker_processes 1;
        error_log logs/error.log warn;
        pid logs/nginx.pid;

        events {{
            worker_connections 4096;
        }}

        http {{
            access_log off;
            keepalive_timeout 65;

            server {{
                listen 127.0.0.1:{port};

                location = /-/ready {{
                    default_type text/plain;
                    return 200 "ready\\n";
                }}

                location = / {{
                    default_type text/plain;
                    return 200 "{body}";
                }}
            }}
        }}
        """
    )


def nginx_proxy_config(port: int, upstream_port: int) -> str:
    return textwrap.dedent(
        f"""\
        worker_processes 1;
        error_log logs/error.log warn;
        pid logs/nginx.pid;

        events {{
            worker_connections 4096;
        }}

        http {{
            access_log off;
            keepalive_timeout 65;

            server {{
                listen 127.0.0.1:{port};

                location = /-/ready {{
                    default_type text/plain;
                    return 200 "ready\\n";
                }}

                location / {{
                    proxy_http_version 1.1;
                    proxy_set_header Connection "";
                    proxy_set_header Host $host;
                    proxy_pass http://127.0.0.1:{upstream_port};
                }}
            }}
        }}
        """
    )


def parse_ab_output(output: str, *, server: str, scenario: str) -> BenchmarkResult:
    def required(pattern: str) -> str:
        match = re.search(pattern, output, re.MULTILINE)
        if match is None:
            raise RuntimeError(f"failed to parse ab output for {server}/{scenario}:\n{output}")
        return match.group(1)

    return BenchmarkResult(
        server=server,
        scenario=scenario,
        tool="ab",
        complete_requests=int(required(r"^Complete requests:\s+(\d+)$")),
        failed_requests=int(required(r"^Failed requests:\s+(\d+)$")),
        requests_per_sec=float(required(r"^Requests per second:\s+([0-9.]+) \[#/sec\] \(mean\)$")),
        time_per_request_ms=float(required(r"^Time per request:\s+([0-9.]+) \[ms\] \(mean\)$")),
        transfer_rate_kb_sec=float(required(r"^Transfer rate:\s+([0-9.]+) \[Kbytes/sec\] received$")),
    )


def benchmark_http(port: int, *, requests: int, concurrency: int) -> BenchmarkResult:
    raise AssertionError("scenario metadata must be bound before benchmark_http is called")


def benchmark_named_http(
    port: int,
    *,
    requests: int,
    concurrency: int,
    server: str,
    scenario: str,
) -> BenchmarkResult:
    completed = run(
        [
            "ab",
            "-k",
            "-q",
            "-n",
            str(requests),
            "-c",
            str(concurrency),
            f"http://127.0.0.1:{port}/",
        ],
        capture_output=True,
    )
    return parse_ab_output(completed.stdout, server=server, scenario=scenario)


def benchmark_named_curl(
    *,
    url: str,
    flags: list[str],
    headers: list[str],
    body: bytes | None,
    requests: int,
    concurrency: int,
    timeout_secs: float,
    server: str,
    scenario: str,
) -> BenchmarkResult:
    durations: list[float] = []
    started = time.perf_counter()
    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as executor:
        futures = [
            executor.submit(
                run_curl_request,
                url=url,
                flags=flags,
                headers=headers,
                body=body,
                timeout_secs=timeout_secs,
            )
            for _ in range(requests)
        ]
        for future in concurrent.futures.as_completed(futures):
            durations.append(future.result())

    wall_elapsed = max(time.perf_counter() - started, 1e-9)
    return BenchmarkResult(
        server=server,
        scenario=scenario,
        tool="curl-threadpool",
        complete_requests=requests,
        failed_requests=0,
        requests_per_sec=round(requests / wall_elapsed, 2),
        time_per_request_ms=round(statistics.mean(durations) * 1000, 3),
        transfer_rate_kb_sec=None,
    )


def grpc_frame(payload: bytes) -> bytes:
    return bytes([0]) + len(payload).to_bytes(4, byteorder="big") + payload


def start_grpc_backend_server(
    port: int,
    *,
    cert_path: pathlib.Path,
    key_path: pathlib.Path,
):
    import grpc

    with cert_path.open("rb") as cert_file:
        cert_pem = cert_file.read()
    with key_path.open("rb") as key_file:
        key_pem = key_file.read()

    server = grpc.server(concurrent.futures.ThreadPoolExecutor(max_workers=16))
    server.add_generic_rpc_handlers(
        (
            grpc.method_handlers_generic_handler(
                "bench.Bench",
                {
                    "Ping": grpc.unary_unary_rpc_method_handler(
                        lambda request, context: request,
                        request_deserializer=lambda payload: payload,
                        response_serializer=lambda payload: payload,
                    )
                },
            ),
        )
    )
    server.add_secure_port(
        f"127.0.0.1:{port}",
        grpc.ssl_server_credentials(((key_pem, cert_pem),)),
    )
    server.start()
    return server


def ensure_rginx_binary(workspace: pathlib.Path) -> pathlib.Path:
    binary = workspace / "target" / "release" / "rginx"
    if binary.exists():
        return binary
    run(["cargo", "build", "--release", "-p", "rginx", "--locked"], cwd=workspace)
    if not binary.exists():
        raise RuntimeError(f"rginx binary was not produced at {binary}")
    return binary


def ensure_nginx_checkout(src_dir: pathlib.Path) -> str:
    if not src_dir.exists():
        run(["git", "clone", "--depth", "1", NGINX_REPO_URL, str(src_dir)])
    commit = run(["git", "rev-parse", "--short", "HEAD"], cwd=src_dir, capture_output=True)
    return commit.stdout.strip()


def ensure_nginx_binary(src_dir: pathlib.Path, install_dir: pathlib.Path) -> pathlib.Path:
    binary = install_dir / "sbin" / "nginx"
    if binary.exists():
        return binary

    configure = [
        "./auto/configure",
        f"--prefix={install_dir}",
        "--with-cc-opt=-O2",
        "--with-http_ssl_module",
        "--with-http_v2_module",
        "--without-http_gzip_module",
    ]
    run(configure, cwd=src_dir)
    jobs = str(max(os.cpu_count() or 1, 1))
    run(["make", "-j", jobs], cwd=src_dir)
    run(["make", "install"], cwd=src_dir)
    if not binary.exists():
        raise RuntimeError(f"nginx binary was not produced at {binary}")
    return binary


def rginx_version(binary: pathlib.Path) -> str:
    completed = run([str(binary), "--version"], capture_output=True)
    return completed.stdout.strip()


def nginx_version(binary: pathlib.Path) -> str:
    completed = subprocess.run(
        [str(binary), "-v"],
        check=False,
        text=True,
        capture_output=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"failed to query nginx version:\nstdout:\n{completed.stdout}\nstderr:\n{completed.stderr}"
        )
    return completed.stderr.strip()


def spawn_process(
    command: list[str],
    *,
    cwd: pathlib.Path | None,
    env: dict[str, str] | None,
    stdout_path: pathlib.Path,
    stderr_path: pathlib.Path,
) -> tuple[subprocess.Popen[str], contextlib.ExitStack]:
    stack = contextlib.ExitStack()
    stdout = stack.enter_context(stdout_path.open("w", encoding="utf-8"))
    stderr = stack.enter_context(stderr_path.open("w", encoding="utf-8"))
    process = subprocess.Popen(
        command,
        cwd=str(cwd) if cwd is not None else None,
        env=env,
        stdout=stdout,
        stderr=stderr,
        text=True,
    )
    return process, stack


def stop_process(process: subprocess.Popen[str], *, name: str) -> None:
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=10)
    if process.returncode not in (0, -15):
        raise RuntimeError(f"{name} exited with unexpected status {process.returncode}")


def measure_reload_apply_time(
    *,
    server_name: str,
    scenario_name: str,
    launch_command: list[str],
    launch_cwd: pathlib.Path | None,
    launch_env: dict[str, str] | None,
    config_path: pathlib.Path,
    reloaded_config: str,
    port: int,
    work_dir: pathlib.Path,
) -> ReloadResult:
    stdout_path = work_dir / f"{server_name}-{scenario_name}.stdout.log"
    stderr_path = work_dir / f"{server_name}-{scenario_name}.stderr.log"
    process, stack = spawn_process(
        launch_command,
        cwd=launch_cwd,
        env=launch_env,
        stdout_path=stdout_path,
        stderr_path=stderr_path,
    )
    try:
        wait_for_ready(port, tls_enabled=False)
        status, body = fetch_text_response(port, "/", tls_enabled=False)
        if status != 200 or body != "old\n":
            raise RuntimeError(
                f"unexpected pre-reload response for {server_name}: status={status} body={body!r}"
            )

        write_text(config_path, reloaded_config)
        started = time.perf_counter()
        os.kill(process.pid, signal.SIGHUP)

        deadline = time.time() + 15.0
        while time.time() < deadline:
            try:
                status, body = fetch_text_response(port, "/", tls_enabled=False)
            except OSError:
                status, body = 0, ""
            if status == 200 and body == "new\n":
                return ReloadResult(
                    server=server_name,
                    scenario=scenario_name,
                    reload_apply_ms=round((time.perf_counter() - started) * 1000, 3),
                )
            time.sleep(0.01)

        raise RuntimeError(f"timed out waiting for {server_name} reload to apply")
    finally:
        stop_process(process, name=f"{server_name}/{scenario_name}")
        stack.close()


def run_single_server_benchmark(
    *,
    server_name: str,
    scenario_name: str,
    launch_command: list[str],
    launch_cwd: pathlib.Path | None,
    launch_env: dict[str, str] | None,
    ready_tls_enabled: bool,
    port: int,
    work_dir: pathlib.Path,
    requests: int,
    concurrency: int,
) -> BenchmarkResult:
    stdout_path = work_dir / f"{server_name}-{scenario_name}.stdout.log"
    stderr_path = work_dir / f"{server_name}-{scenario_name}.stderr.log"
    process, stack = spawn_process(
        launch_command,
        cwd=launch_cwd,
        env=launch_env,
        stdout_path=stdout_path,
        stderr_path=stderr_path,
    )
    try:
        wait_for_ready(port, tls_enabled=ready_tls_enabled)
        return benchmark_named_http(
            port,
            requests=requests,
            concurrency=concurrency,
            server=server_name,
            scenario=scenario_name,
        )
    finally:
        stop_process(process, name=f"{server_name}/{scenario_name}")
        stack.close()


def run_single_server_curl_benchmark(
    *,
    server_name: str,
    scenario_name: str,
    launch_command: list[str],
    launch_cwd: pathlib.Path | None,
    launch_env: dict[str, str] | None,
    ready_tls_enabled: bool,
    port: int,
    work_dir: pathlib.Path,
    url: str,
    flags: list[str],
    headers: list[str],
    body: bytes | None,
    timeout_secs: float,
    requests: int,
    concurrency: int,
) -> BenchmarkResult:
    stdout_path = work_dir / f"{server_name}-{scenario_name}.stdout.log"
    stderr_path = work_dir / f"{server_name}-{scenario_name}.stderr.log"
    process, stack = spawn_process(
        launch_command,
        cwd=launch_cwd,
        env=launch_env,
        stdout_path=stdout_path,
        stderr_path=stderr_path,
    )
    try:
        wait_for_ready(port, tls_enabled=ready_tls_enabled)
        return benchmark_named_curl(
            url=url,
            flags=flags,
            headers=headers,
            body=body,
            timeout_secs=timeout_secs,
            requests=requests,
            concurrency=concurrency,
            server=server_name,
            scenario=scenario_name,
        )
    finally:
        stop_process(process, name=f"{server_name}/{scenario_name}")
        stack.close()


def write_text(path: pathlib.Path, contents: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(contents, encoding="utf-8")


def ratio_rows(results: list[BenchmarkResult]) -> list[dict[str, str]]:
    grouped: dict[str, dict[str, BenchmarkResult]] = {}
    for result in results:
        grouped.setdefault(result.scenario, {})[result.server] = result

    rows: list[dict[str, str]] = []
    for scenario in sorted(grouped):
        servers = grouped[scenario]
        if "rginx" not in servers or "nginx" not in servers:
            continue
        rginx_result = servers["rginx"]
        nginx_result = servers["nginx"]
        ratio = (
            rginx_result.requests_per_sec / nginx_result.requests_per_sec
            if nginx_result.requests_per_sec
            else 0.0
        )
        rows.append(
            {
                "scenario": scenario,
                "rginx_rps": f"{rginx_result.requests_per_sec:.2f}",
                "nginx_rps": f"{nginx_result.requests_per_sec:.2f}",
                "rginx_div_nginx": f"{ratio:.3f}",
            }
        )
    return rows


def render_markdown(
    *,
    results: list[BenchmarkResult],
    unsupported: list[UnsupportedScenario],
    reload_results: list[ReloadResult],
    requests: int,
    concurrency: int,
    rginx_version_text: str,
    nginx_version_text: str,
    nginx_commit: str,
) -> str:
    lines = [
        "# rginx vs nginx performance snapshot",
        "",
        f"- Environment: Docker / Debian trixie",
        f"- Benchmark tools: `ab -k -n {requests} -c {concurrency}` for plain HTTP/1.1, `curl` threadpool for TLS / HTTP2 / gRPC / grpc-web",
        f"- rginx: `{rginx_version_text}`",
        f"- nginx: `{nginx_version_text}` (source commit `{nginx_commit}`)",
        "- nginx build flags: `--without-http_gzip_module`",
        "",
        "## Raw results",
        "",
        "| scenario | server | tool | complete | failed | req/s | mean ms | KB/s |",
        "| --- | --- | --- | ---: | ---: | ---: | ---: | ---: |",
    ]

    for result in sorted(results, key=lambda item: (item.scenario, item.server)):
        lines.append(
            f"| {result.scenario} | {result.server} | {result.tool} | {result.complete_requests} | "
            f"{result.failed_requests} | {result.requests_per_sec:.2f} | "
            f"{result.time_per_request_ms:.3f} | "
            f"{'-' if result.transfer_rate_kb_sec is None else f'{result.transfer_rate_kb_sec:.2f}'} |"
        )

    lines.extend(
        [
            "",
            "## Throughput ratio",
            "",
            "| scenario | rginx req/s | nginx req/s | rginx/nginx |",
            "| --- | ---: | ---: | ---: |",
        ]
    )

    for row in ratio_rows(results):
        lines.append(
            f"| {row['scenario']} | {row['rginx_rps']} | {row['nginx_rps']} | {row['rginx_div_nginx']} |"
        )

    if unsupported:
        lines.extend(
            [
                "",
                "## Unsupported",
                "",
                "| scenario | server | reason |",
                "| --- | --- | --- |",
            ]
        )
        for item in sorted(unsupported, key=lambda value: (value.scenario, value.server)):
            lines.append(f"| {item.scenario} | {item.server} | {item.reason} |")

    if reload_results:
        lines.extend(
            [
                "",
                "## Reload",
                "",
                "| scenario | server | reload_apply_ms |",
                "| --- | --- | ---: |",
            ]
        )
        for item in sorted(reload_results, key=lambda value: (value.scenario, value.server)):
            lines.append(f"| {item.scenario} | {item.server} | {item.reload_apply_ms:.3f} |")

    lines.extend(
        [
            "",
            "## Notes",
            "",
            "- The goal is repeatable relative comparison inside the same trixie container, not a universal headline benchmark.",
            "- HTTP/1.1 direct/proxy scenarios use `ab`; TLS, HTTP/2, gRPC, and grpc-web scenarios use concurrent `curl` processes.",
            "- `grpc-web` is currently benchmarked only for `rginx`; `NGINX OSS` is recorded as unsupported because it has no native grpc-web translation path.",
            "- Add larger samples, repeated runs, TLS upstream verification variants, RSS/CPU sampling, and reload drain behavior before turning these numbers into external claims.",
        ]
    )
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Compare rginx and nginx inside a reproducible local harness."
    )
    parser.add_argument("--workspace", type=pathlib.Path, required=True)
    parser.add_argument("--out-dir", type=pathlib.Path, required=True)
    parser.add_argument("--requests", type=int, default=5000)
    parser.add_argument("--concurrency", type=int, default=64)
    parser.add_argument("--nginx-src-dir", type=pathlib.Path, default=None)
    parser.add_argument("--nginx-install-dir", type=pathlib.Path, default=None)
    args = parser.parse_args()

    workspace = args.workspace.resolve()
    out_dir = args.out_dir.resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    build_root = pathlib.Path(tempfile.mkdtemp(prefix="rginx-nginx-compare-"))
    try:
        nginx_src_dir = (
            args.nginx_src_dir.resolve()
            if args.nginx_src_dir is not None
            else build_root / "nginx-src"
        )
        nginx_install_dir = (
            args.nginx_install_dir.resolve()
            if args.nginx_install_dir is not None
            else build_root / "nginx-install"
        )

        rginx_bin = ensure_rginx_binary(workspace)
        nginx_commit = ensure_nginx_checkout(nginx_src_dir)
        nginx_bin = ensure_nginx_binary(nginx_src_dir, nginx_install_dir)

        rginx_version_text = rginx_version(rginx_bin)
        nginx_version_text = nginx_version(nginx_bin)

        cert_dir = build_root / "certs"
        cert_dir.mkdir(parents=True, exist_ok=True)
        frontend_cert = cert_dir / "frontend.crt"
        frontend_key = cert_dir / "frontend.key"
        backend_cert = cert_dir / "backend.crt"
        backend_key = cert_dir / "backend.key"
        generate_self_signed_cert(frontend_cert, frontend_key)
        generate_self_signed_cert(backend_cert, backend_key)

        upstream_server, upstream_thread, upstream_port = start_upstream_server()
        grpc_backend_port = reserve_port()
        grpc_backend_server = start_grpc_backend_server(
            grpc_backend_port,
            cert_path=backend_cert,
            key_path=backend_key,
        )
        try:
            port_plan = {
                "return": {"rginx": reserve_port(), "nginx": reserve_port()},
                "proxy": {"rginx": reserve_port(), "nginx": reserve_port()},
                "https_return": {"rginx": reserve_port(), "nginx": reserve_port()},
                "grpc": {"rginx": reserve_port(), "nginx": reserve_port()},
                "reload": {"rginx": reserve_port(), "nginx": reserve_port()},
            }

            rginx_return_path = build_root / "rginx-return.ron"
            rginx_proxy_path = build_root / "rginx-proxy.ron"
            rginx_tls_return_path = build_root / "rginx-tls-return.ron"
            rginx_grpc_path = build_root / "rginx-grpc.ron"
            rginx_reload_path = build_root / "rginx-reload.ron"
            nginx_return_dir = build_root / "nginx-return"
            nginx_proxy_dir = build_root / "nginx-proxy"
            nginx_tls_return_dir = build_root / "nginx-tls-return"
            nginx_grpc_dir = build_root / "nginx-grpc"
            nginx_reload_dir = build_root / "nginx-reload"
            for directory in [
                nginx_return_dir,
                nginx_proxy_dir,
                nginx_tls_return_dir,
                nginx_grpc_dir,
                nginx_reload_dir,
            ]:
                directory.mkdir(parents=True, exist_ok=True)
                (directory / "logs").mkdir(parents=True, exist_ok=True)

            rginx_env = os.environ.copy()
            rginx_env["RUST_LOG"] = "warn"
            nginx_env = os.environ.copy()

            write_text(rginx_return_path, rginx_return_config(port_plan["return"]["rginx"]))
            write_text(
                rginx_proxy_path,
                rginx_proxy_config(port_plan["proxy"]["rginx"], upstream_port),
            )
            write_text(
                rginx_tls_return_path,
                rginx_tls_return_config(
                    port_plan["https_return"]["rginx"],
                    frontend_cert,
                    frontend_key,
                ),
            )
            write_text(
                rginx_grpc_path,
                rginx_grpc_proxy_config(
                    port_plan["grpc"]["rginx"],
                    frontend_cert,
                    frontend_key,
                    grpc_backend_port,
                ),
            )
            write_text(rginx_reload_path, rginx_reload_config(port_plan["reload"]["rginx"], "old\\n"))
            write_text(
                nginx_return_dir / "nginx.conf",
                nginx_return_config(port_plan["return"]["nginx"]),
            )
            write_text(
                nginx_proxy_dir / "nginx.conf",
                nginx_proxy_config(port_plan["proxy"]["nginx"], upstream_port),
            )
            write_text(
                nginx_tls_return_dir / "nginx.conf",
                nginx_tls_return_config(
                    port_plan["https_return"]["nginx"],
                    frontend_cert,
                    frontend_key,
                ),
            )
            write_text(
                nginx_grpc_dir / "nginx.conf",
                nginx_grpc_proxy_config(
                    port_plan["grpc"]["nginx"],
                    frontend_cert,
                    frontend_key,
                    grpc_backend_port,
                ),
            )
            write_text(nginx_reload_dir / "nginx.conf", nginx_reload_config(port_plan["reload"]["nginx"], "old\\n"))

            results: list[BenchmarkResult] = []
            unsupported: list[UnsupportedScenario] = []
            reload_results: list[ReloadResult] = []

            results.append(
                run_single_server_benchmark(
                    server_name="rginx",
                    scenario_name="return_200",
                    launch_command=[str(rginx_bin), "--config", str(rginx_return_path)],
                    launch_cwd=workspace,
                    launch_env=rginx_env,
                    ready_tls_enabled=False,
                    port=port_plan["return"]["rginx"],
                    work_dir=out_dir,
                    requests=args.requests,
                    concurrency=args.concurrency,
                )
            )
            results.append(
                run_single_server_benchmark(
                    server_name="nginx",
                    scenario_name="return_200",
                    launch_command=[
                        str(nginx_bin),
                        "-p",
                        str(nginx_return_dir),
                        "-c",
                        str(nginx_return_dir / "nginx.conf"),
                        "-g",
                        "daemon off;",
                    ],
                    launch_cwd=nginx_return_dir,
                    launch_env=nginx_env,
                    ready_tls_enabled=False,
                    port=port_plan["return"]["nginx"],
                    work_dir=out_dir,
                    requests=args.requests,
                    concurrency=args.concurrency,
                )
            )
            results.append(
                run_single_server_benchmark(
                    server_name="rginx",
                    scenario_name="proxy_http1",
                    launch_command=[str(rginx_bin), "--config", str(rginx_proxy_path)],
                    launch_cwd=workspace,
                    launch_env=rginx_env,
                    ready_tls_enabled=False,
                    port=port_plan["proxy"]["rginx"],
                    work_dir=out_dir,
                    requests=args.requests,
                    concurrency=args.concurrency,
                )
            )
            results.append(
                run_single_server_benchmark(
                    server_name="nginx",
                    scenario_name="proxy_http1",
                    launch_command=[
                        str(nginx_bin),
                        "-p",
                        str(nginx_proxy_dir),
                        "-c",
                        str(nginx_proxy_dir / "nginx.conf"),
                        "-g",
                        "daemon off;",
                    ],
                    launch_cwd=nginx_proxy_dir,
                    launch_env=nginx_env,
                    ready_tls_enabled=False,
                    port=port_plan["proxy"]["nginx"],
                    work_dir=out_dir,
                    requests=args.requests,
                    concurrency=args.concurrency,
                )
            )

            https_flags = ["--http1.1", "--insecure"]
            http2_flags = ["--http2", "--insecure"]
            grpc_flags = ["--http2", "--insecure", "--request", "POST", "--data-binary", "@-"]
            grpc_headers = ["content-type: application/grpc", "te: trailers"]
            grpc_body = grpc_frame(b"ping")
            grpc_web_flags = ["--http1.1", "--insecure", "--request", "POST", "--data-binary", "@-"]

            results.append(
                run_single_server_curl_benchmark(
                    server_name="rginx",
                    scenario_name="https_return_200",
                    launch_command=[str(rginx_bin), "--config", str(rginx_tls_return_path)],
                    launch_cwd=workspace,
                    launch_env=rginx_env,
                    ready_tls_enabled=True,
                    port=port_plan["https_return"]["rginx"],
                    work_dir=out_dir,
                    url=f"https://127.0.0.1:{port_plan['https_return']['rginx']}/",
                    flags=https_flags,
                    headers=[],
                    body=None,
                    timeout_secs=10.0,
                    requests=args.requests,
                    concurrency=args.concurrency,
                )
            )
            results.append(
                run_single_server_curl_benchmark(
                    server_name="nginx",
                    scenario_name="https_return_200",
                    launch_command=[
                        str(nginx_bin),
                        "-p",
                        str(nginx_tls_return_dir),
                        "-c",
                        str(nginx_tls_return_dir / "nginx.conf"),
                        "-g",
                        "daemon off;",
                    ],
                    launch_cwd=nginx_tls_return_dir,
                    launch_env=nginx_env,
                    ready_tls_enabled=True,
                    port=port_plan["https_return"]["nginx"],
                    work_dir=out_dir,
                    url=f"https://127.0.0.1:{port_plan['https_return']['nginx']}/",
                    flags=https_flags,
                    headers=[],
                    body=None,
                    timeout_secs=10.0,
                    requests=args.requests,
                    concurrency=args.concurrency,
                )
            )
            results.append(
                run_single_server_curl_benchmark(
                    server_name="rginx",
                    scenario_name="http2_tls_return_200",
                    launch_command=[str(rginx_bin), "--config", str(rginx_tls_return_path)],
                    launch_cwd=workspace,
                    launch_env=rginx_env,
                    ready_tls_enabled=True,
                    port=port_plan["https_return"]["rginx"],
                    work_dir=out_dir,
                    url=f"https://127.0.0.1:{port_plan['https_return']['rginx']}/",
                    flags=http2_flags,
                    headers=[],
                    body=None,
                    timeout_secs=10.0,
                    requests=args.requests,
                    concurrency=args.concurrency,
                )
            )
            results.append(
                run_single_server_curl_benchmark(
                    server_name="nginx",
                    scenario_name="http2_tls_return_200",
                    launch_command=[
                        str(nginx_bin),
                        "-p",
                        str(nginx_tls_return_dir),
                        "-c",
                        str(nginx_tls_return_dir / "nginx.conf"),
                        "-g",
                        "daemon off;",
                    ],
                    launch_cwd=nginx_tls_return_dir,
                    launch_env=nginx_env,
                    ready_tls_enabled=True,
                    port=port_plan["https_return"]["nginx"],
                    work_dir=out_dir,
                    url=f"https://127.0.0.1:{port_plan['https_return']['nginx']}/",
                    flags=http2_flags,
                    headers=[],
                    body=None,
                    timeout_secs=10.0,
                    requests=args.requests,
                    concurrency=args.concurrency,
                )
            )
            results.append(
                run_single_server_curl_benchmark(
                    server_name="rginx",
                    scenario_name="grpc_unary",
                    launch_command=[str(rginx_bin), "--config", str(rginx_grpc_path)],
                    launch_cwd=workspace,
                    launch_env=rginx_env,
                    ready_tls_enabled=True,
                    port=port_plan["grpc"]["rginx"],
                    work_dir=out_dir,
                    url=f"https://127.0.0.1:{port_plan['grpc']['rginx']}/bench.Bench/Ping",
                    flags=grpc_flags,
                    headers=grpc_headers,
                    body=grpc_body,
                    timeout_secs=10.0,
                    requests=args.requests,
                    concurrency=args.concurrency,
                )
            )
            results.append(
                run_single_server_curl_benchmark(
                    server_name="nginx",
                    scenario_name="grpc_unary",
                    launch_command=[
                        str(nginx_bin),
                        "-p",
                        str(nginx_grpc_dir),
                        "-c",
                        str(nginx_grpc_dir / "nginx.conf"),
                        "-g",
                        "daemon off;",
                    ],
                    launch_cwd=nginx_grpc_dir,
                    launch_env=nginx_env,
                    ready_tls_enabled=True,
                    port=port_plan["grpc"]["nginx"],
                    work_dir=out_dir,
                    url=f"https://127.0.0.1:{port_plan['grpc']['nginx']}/bench.Bench/Ping",
                    flags=grpc_flags,
                    headers=grpc_headers,
                    body=grpc_body,
                    timeout_secs=10.0,
                    requests=args.requests,
                    concurrency=args.concurrency,
                )
            )
            results.append(
                run_single_server_curl_benchmark(
                    server_name="rginx",
                    scenario_name="grpc_web_binary",
                    launch_command=[str(rginx_bin), "--config", str(rginx_grpc_path)],
                    launch_cwd=workspace,
                    launch_env=rginx_env,
                    ready_tls_enabled=True,
                    port=port_plan["grpc"]["rginx"],
                    work_dir=out_dir,
                    url=f"https://127.0.0.1:{port_plan['grpc']['rginx']}/bench.Bench/Ping",
                    flags=grpc_web_flags,
                    headers=["content-type: application/grpc-web+proto", "x-grpc-web: 1"],
                    body=grpc_body,
                    timeout_secs=10.0,
                    requests=args.requests,
                    concurrency=args.concurrency,
                )
            )
            results.append(
                run_single_server_curl_benchmark(
                    server_name="rginx",
                    scenario_name="grpc_web_text",
                    launch_command=[str(rginx_bin), "--config", str(rginx_grpc_path)],
                    launch_cwd=workspace,
                    launch_env=rginx_env,
                    ready_tls_enabled=True,
                    port=port_plan["grpc"]["rginx"],
                    work_dir=out_dir,
                    url=f"https://127.0.0.1:{port_plan['grpc']['rginx']}/bench.Bench/Ping",
                    flags=grpc_web_flags,
                    headers=["content-type: application/grpc-web-text+proto", "x-grpc-web: 1"],
                    body=base64.b64encode(grpc_body),
                    timeout_secs=10.0,
                    requests=args.requests,
                    concurrency=args.concurrency,
                )
            )

            unsupported.extend(
                [
                    UnsupportedScenario(
                        server="nginx",
                        scenario="grpc_web_binary",
                        reason="NGINX OSS has no native grpc-web translation path",
                    ),
                    UnsupportedScenario(
                        server="nginx",
                        scenario="grpc_web_text",
                        reason="NGINX OSS has no native grpc-web translation path",
                    ),
                ]
            )

            reload_results.append(
                measure_reload_apply_time(
                    server_name="rginx",
                    scenario_name="reload_return_body",
                    launch_command=[str(rginx_bin), "--config", str(rginx_reload_path)],
                    launch_cwd=workspace,
                    launch_env=rginx_env,
                    config_path=rginx_reload_path,
                    reloaded_config=rginx_reload_config(port_plan["reload"]["rginx"], "new\\n"),
                    port=port_plan["reload"]["rginx"],
                    work_dir=out_dir,
                )
            )
            reload_results.append(
                measure_reload_apply_time(
                    server_name="nginx",
                    scenario_name="reload_return_body",
                    launch_command=[
                        str(nginx_bin),
                        "-p",
                        str(nginx_reload_dir),
                        "-c",
                        str(nginx_reload_dir / "nginx.conf"),
                        "-g",
                        "daemon off;",
                    ],
                    launch_cwd=nginx_reload_dir,
                    launch_env=nginx_env,
                    config_path=nginx_reload_dir / "nginx.conf",
                    reloaded_config=nginx_reload_config(port_plan["reload"]["nginx"], "new\\n"),
                    port=port_plan["reload"]["nginx"],
                    work_dir=out_dir,
                )
            )
        finally:
            upstream_server.shutdown()
            upstream_server.server_close()
            upstream_thread.join(timeout=5)
            grpc_backend_server.stop(0).wait()

        payload = {
            "environment": "docker-trixie",
            "requests": args.requests,
            "concurrency": args.concurrency,
            "rginx_version": rginx_version_text,
            "nginx_version": nginx_version_text,
            "nginx_commit": nginx_commit,
            "results": [dataclasses.asdict(result) for result in results],
            "unsupported": [dataclasses.asdict(item) for item in unsupported],
            "reload": [dataclasses.asdict(item) for item in reload_results],
        }
        write_text(out_dir / "performance-results.json", json.dumps(payload, indent=2) + "\n")
        write_text(
            out_dir / "performance-results.md",
            render_markdown(
                results=results,
                unsupported=unsupported,
                reload_results=reload_results,
                requests=args.requests,
                concurrency=args.concurrency,
                rginx_version_text=rginx_version_text,
                nginx_version_text=nginx_version_text,
                nginx_commit=nginx_commit,
            ),
        )
        print((out_dir / "performance-results.md").read_text(encoding="utf-8"))
        return 0
    finally:
        shutil.rmtree(build_root, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
