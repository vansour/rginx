from __future__ import annotations

import base64
import concurrent.futures
import contextlib
import http.server
import os
import pathlib
import re
import signal
import socket
import ssl
import statistics
import subprocess
import tempfile
import threading
import time

from common import BenchmarkResult, ReloadResult, ReservedPort, port_number, run, write_text


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
    from common import reserve_port

    reserved = reserve_port()
    port = reserved.port
    reserved.release()
    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), UpstreamHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread, port


def fetch_text_response(port: int | ReservedPort, path: str, *, tls_enabled: bool) -> tuple[int, str]:
    port = port_number(port)
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


def wait_for_ready(port: int | ReservedPort, *, tls_enabled: bool, timeout_secs: float = 20.0) -> None:
    port = port_number(port)
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
    grpc_mode: str | None = None,
) -> float:
    started = time.perf_counter()
    with tempfile.NamedTemporaryFile() as headers_file:
        command = [
            "curl",
            "--silent",
            "--show-error",
            "--fail",
            "--output",
            "-",
            "--dump-header",
            headers_file.name,
            "--max-time",
            str(timeout_secs),
            *flags,
        ]
        for header in headers:
            command.extend(["--header", header])
        command.append(url)
        completed = subprocess.run(command, input=body, check=True, capture_output=True)
        if grpc_mode is not None:
            grpc_status = extract_grpc_status(
                header_bytes=pathlib.Path(headers_file.name).read_bytes(),
                body_bytes=completed.stdout,
                grpc_mode=grpc_mode,
            )
            if grpc_status is None:
                raise RuntimeError(f"missing grpc-status for {url}")
            if grpc_status != "0":
                raise RuntimeError(f"unexpected grpc-status {grpc_status} for {url}")
    return time.perf_counter() - started


def extract_grpc_status(*, header_bytes: bytes, body_bytes: bytes, grpc_mode: str) -> str | None:
    header_matches = re.findall(rb"(?im)^grpc-status:\s*([0-9]+)\s*$", header_bytes)
    if header_matches:
        return header_matches[-1].decode("ascii")

    if grpc_mode == "grpc-web-text":
        try:
            body_bytes = base64.b64decode(body_bytes, validate=False)
        except Exception:
            return None

    body_matches = re.findall(rb"grpc-status:\s*([0-9]+)", body_bytes)
    if body_matches:
        return body_matches[-1].decode("ascii")
    return None


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
    grpc_mode: str | None = None,
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
                grpc_mode=grpc_mode,
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
        if process.returncode not in (0, -15):
            raise RuntimeError(f"{name} exited with unexpected status {process.returncode}")
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
    port: int | ReservedPort,
    work_dir: pathlib.Path,
) -> ReloadResult:
    port = port_number(port, release=True)
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
    port: int | ReservedPort,
    work_dir: pathlib.Path,
    requests: int,
    concurrency: int,
) -> BenchmarkResult:
    port = port_number(port, release=True)
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
    port: int | ReservedPort,
    work_dir: pathlib.Path,
    url: str,
    flags: list[str],
    headers: list[str],
    body: bytes | None,
    timeout_secs: float,
    requests: int,
    concurrency: int,
    grpc_mode: str | None = None,
) -> BenchmarkResult:
    port = port_number(port, release=True)
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
            grpc_mode=grpc_mode,
            requests=requests,
            concurrency=concurrency,
            server=server_name,
            scenario=scenario_name,
        )
    finally:
        stop_process(process, name=f"{server_name}/{scenario_name}")
        stack.close()
