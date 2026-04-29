#!/usr/bin/env python3
from __future__ import annotations

import argparse
import concurrent.futures
import http.client
import http.server
import socket
import socketserver
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from collections import Counter
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import urlsplit


READY_ROUTE_CONFIG = """        LocationConfig(
            matcher: Exact("/-/ready"),
            handler: Return(
                status: 200,
                location: "",
                body: Some("ready\\n"),
            ),
        ),
"""

REVALIDATE_ETAG = '"cache-bench-etag"'
STARTUP_RETRY_LIMIT = 8


@dataclass(frozen=True)
class BenchRequest:
    path: str
    expected_status: int
    expected_cache: str
    expected_length: int
    headers: dict[str, str]


@dataclass(frozen=True)
class ScenarioRow:
    name: str
    requests: int
    concurrency: int
    expected_cache: str
    upstream_requests: int
    elapsed_s: float
    req_per_sec: float
    avg_ms: float
    p95_ms: float


class ThreadingHttpServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


class OriginState:
    def __init__(self, body_bytes: int, slice_payload_bytes: int) -> None:
        self.lock = threading.Lock()
        self.counts = Counter()
        self.body = (b"x" * body_bytes) or b"x"
        payload = bytearray()
        alphabet = b"abcdefghijklmnopqrstuvwxyz"
        while len(payload) < slice_payload_bytes:
            payload.extend(alphabet)
        self.slice_payload = bytes(payload[:slice_payload_bytes])

    def bump(self, key: str) -> None:
        with self.lock:
            self.counts[key] += 1

    def count(self, key: str) -> int:
        with self.lock:
            return self.counts[key]


class OriginHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, format: str, *args: object) -> None:
        return

    def do_GET(self) -> None:  # noqa: N802
        path = urlsplit(self.path).path or self.path.split("?", 1)[0]
        state: OriginState = self.server.state  # type: ignore[attr-defined]
        if path.startswith("/fill/"):
            state.bump("fill")
            self.respond(200, state.body, {"Cache-Control": "max-age=60"})
            return
        if path.startswith("/hit/"):
            state.bump("hit")
            self.respond(200, state.body, {"Cache-Control": "max-age=60"})
            return
        if path == "/revalidate":
            state.bump("revalidate")
            headers = {
                "Cache-Control": "max-age=60, no-cache",
                "ETag": REVALIDATE_ETAG,
            }
            if self.headers.get("If-None-Match") == REVALIDATE_ETAG:
                self.respond(304, b"", headers)
            else:
                self.respond(200, state.body, headers)
            return
        if path == "/slice":
            state.bump("slice")
            self.respond_range(state.slice_payload)
            return
        self.respond(404, b"not found\n", {"Cache-Control": "no-store"})

    def respond_range(self, payload: bytes) -> None:
        range_header = self.headers.get("Range")
        if not range_header:
            self.respond(200, payload, {"Cache-Control": "max-age=60"})
            return

        parsed = parse_single_range(range_header, len(payload))
        if parsed is None:
            self.respond(416, b"", {"Content-Range": f"bytes */{len(payload)}"})
            return

        start, end = parsed
        body = payload[start : end + 1]
        self.respond(
            206,
            body,
            {
                "Cache-Control": "max-age=60",
                "Content-Range": f"bytes {start}-{end}/{len(payload)}",
            },
        )

    def respond(self, status: int, body: bytes, headers: dict[str, str]) -> None:
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        for name, value in headers.items():
            self.send_header(name, value)
        self.end_headers()
        if body:
            self.wfile.write(body)


def parse_single_range(header_value: str, payload_len: int) -> tuple[int, int] | None:
    value = header_value.strip()
    if not value.startswith("bytes="):
        return None
    raw_range = value[len("bytes=") :].strip()
    if "," in raw_range or "-" not in raw_range:
        return None
    raw_start, raw_end = raw_range.split("-", 1)
    raw_start = raw_start.strip()
    raw_end = raw_end.strip()
    if not raw_start and not raw_end:
        return None
    try:
        if raw_start:
            start = int(raw_start)
            if start < 0 or start >= payload_len:
                return None
            if raw_end:
                end = int(raw_end)
                if end < start:
                    return None
            else:
                end = payload_len - 1
            return start, min(end, payload_len - 1)

        suffix_len = int(raw_end)
    except ValueError:
        return None
    if suffix_len <= 0:
        return None
    suffix_len = min(suffix_len, payload_len)
    return payload_len - suffix_len, payload_len - 1


def reserve_loopback_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def ensure_rginx_binary(root: Path, explicit: str | None, rebuild: bool) -> Path:
    if explicit:
        return Path(explicit).resolve()

    binary = root / "target" / "release" / "rginx"
    if rebuild or not binary.exists():
        subprocess.run(
            ["cargo", "build", "--release", "--locked", "-p", "rginx"],
            cwd=root,
            check=True,
        )
    return binary


def write_proxy_config(
    path: Path,
    listen_port: int,
    upstream_port: int,
    cache_dir: Path,
    cache_max_size_bytes: int,
    max_entry_bytes: int,
    slice_size_bytes: int,
) -> None:
    config = f"""Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 2,
    ),
    cache_zones: [
        CacheZoneConfig(
            name: "default",
            path: "{cache_dir.as_posix()}",
            max_size_bytes: Some({cache_max_size_bytes}),
            inactive_secs: Some(600),
            default_ttl_secs: Some(60),
            max_entry_bytes: Some({max_entry_bytes}),
        ),
    ],
    server: ServerConfig(
        listen: "127.0.0.1:{listen_port}",
    ),
    upstreams: [
        UpstreamConfig(
            name: "backend",
            peers: [
                UpstreamPeerConfig(
                    url: "http://127.0.0.1:{upstream_port}",
                ),
            ],
            request_timeout_secs: Some(5),
        ),
    ],
    locations: [
{READY_ROUTE_CONFIG}        LocationConfig(
            matcher: Prefix("/fill"),
            handler: Proxy(
                upstream: "backend",
            ),
            cache: Some(CacheRouteConfig(
                zone: "default",
                methods: Some(["GET", "HEAD"]),
                statuses: Some([200]),
                key: Some("{{scheme}}:{{host}}:{{uri}}"),
                stale_if_error_secs: Some(60),
            )),
        ),
        LocationConfig(
            matcher: Prefix("/hit"),
            handler: Proxy(
                upstream: "backend",
            ),
            cache: Some(CacheRouteConfig(
                zone: "default",
                methods: Some(["GET", "HEAD"]),
                statuses: Some([200]),
                key: Some("{{scheme}}:{{host}}:{{uri}}"),
                stale_if_error_secs: Some(60),
            )),
        ),
        LocationConfig(
            matcher: Exact("/revalidate"),
            handler: Proxy(
                upstream: "backend",
            ),
            cache: Some(CacheRouteConfig(
                zone: "default",
                methods: Some(["GET", "HEAD"]),
                statuses: Some([200]),
                key: Some("{{scheme}}:{{host}}:{{uri}}"),
                stale_if_error_secs: Some(60),
            )),
        ),
        LocationConfig(
            matcher: Exact("/slice"),
            handler: Proxy(
                upstream: "backend",
            ),
            cache: Some(CacheRouteConfig(
                zone: "default",
                methods: Some(["GET", "HEAD"]),
                statuses: Some([206]),
                key: Some("{{scheme}}:{{host}}:{{uri}}"),
                stale_if_error_secs: Some(60),
                range_requests: Some(Cache),
                slice_size_bytes: Some({slice_size_bytes}),
            )),
        ),
    ],
)
"""
    path.write_text(config, encoding="utf-8")


def read_process_log(log_path: Path) -> str:
    if not log_path.exists():
        return ""
    return log_path.read_text(encoding="utf-8", errors="replace")


def should_retry_startup(log_output: str) -> bool:
    lowered = log_output.lower()
    return (
        "address already in use" in lowered
        or "os error 98" in lowered
        or "addrinuse" in lowered
    )


def wait_for_ready(
    port: int,
    timeout: float,
    process: subprocess.Popen[str],
    log_path: Path,
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            output = read_process_log(log_path)
            raise RuntimeError(
                f"rginx exited before becoming ready on 127.0.0.1:{port}:\n{output}"
            )
        try:
            conn = http.client.HTTPConnection("127.0.0.1", port, timeout=0.5)
            conn.request("GET", "/-/ready", headers={"Host": f"127.0.0.1:{port}"})
            response = conn.getresponse()
            response.read()
            conn.close()
            if response.status == 200:
                return
        except OSError:
            time.sleep(0.1)
        else:
            time.sleep(0.1)
    raise TimeoutError(f"timed out waiting for rginx to listen on 127.0.0.1:{port}")


def estimate_cache_max_size_bytes(
    max_entry_bytes: int,
    fill_keys: int,
    hit_keys: int,
) -> int:
    expected_cached_entries = fill_keys + hit_keys + 2
    return max_entry_bytes * max(expected_cached_entries * 4, 128)


def start_rginx(
    root: Path,
    binary: Path,
    config_path: Path,
    log_path: Path,
    origin_port: int,
    cache_dir: Path,
    cache_max_size_bytes: int,
    max_entry_bytes: int,
    slice_size_bytes: int,
    ready_timeout: float,
) -> tuple[subprocess.Popen[str], int]:
    last_error: Exception | None = None
    for attempt in range(1, STARTUP_RETRY_LIMIT + 1):
        listen_port = reserve_loopback_port()
        write_proxy_config(
            config_path,
            listen_port,
            origin_port,
            cache_dir,
            cache_max_size_bytes,
            max_entry_bytes,
            slice_size_bytes,
        )
        with log_path.open("w", encoding="utf-8") as log_file:
            process = subprocess.Popen(
                [str(binary), "--config", str(config_path)],
                cwd=root,
                stdout=log_file,
                stderr=subprocess.STDOUT,
                text=True,
            )
        try:
            wait_for_ready(listen_port, ready_timeout, process, log_path)
            return process, listen_port
        except RuntimeError as error:
            last_error = error
            output = read_process_log(log_path)
            terminate_process(process)
            if attempt < STARTUP_RETRY_LIMIT and should_retry_startup(output):
                continue
            raise
        except Exception as error:
            last_error = error
            terminate_process(process)
            raise
    assert last_error is not None
    raise last_error


def run_request(port: int, bench_request: BenchRequest, timeout: float) -> float:
    started = time.perf_counter()
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    headers = {"Host": f"127.0.0.1:{port}", "Connection": "close", **bench_request.headers}
    conn.request("GET", bench_request.path, headers=headers)
    response = conn.getresponse()
    body = response.read()
    x_cache = response.getheader("x-cache")
    conn.close()

    if response.status != bench_request.expected_status:
        raise RuntimeError(
            f"{bench_request.path} returned {response.status}, "
            f"expected {bench_request.expected_status}"
        )
    if x_cache != bench_request.expected_cache:
        raise RuntimeError(
            f"{bench_request.path} returned x-cache={x_cache!r}, "
            f"expected {bench_request.expected_cache!r}"
        )
    if len(body) != bench_request.expected_length:
        raise RuntimeError(
            f"{bench_request.path} returned body length {len(body)}, "
            f"expected {bench_request.expected_length}"
        )

    return time.perf_counter() - started


def benchmark(port: int, requests: list[BenchRequest], concurrency: int, timeout: float) -> tuple[float, float, float, float]:
    started = time.perf_counter()
    durations: list[float] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as executor:
        futures = [executor.submit(run_request, port, request, timeout) for request in requests]
        for future in concurrent.futures.as_completed(futures):
            durations.append(future.result())
    elapsed = time.perf_counter() - started
    avg_ms = statistics.mean(durations) * 1000 if durations else 0.0
    req_per_sec = len(requests) / elapsed if elapsed else 0.0
    p95_ms = percentile_ms(durations, 0.95)
    return elapsed, req_per_sec, avg_ms, p95_ms


def percentile_ms(durations: list[float], percentile: float) -> float:
    if not durations:
        return 0.0
    ordered = sorted(durations)
    index = max(0, min(len(ordered) - 1, int(round((len(ordered) - 1) * percentile))))
    return ordered[index] * 1000


def fill_requests(count: int, body_bytes: int) -> list[BenchRequest]:
    return [
        BenchRequest(
            path=f"/fill/{request_id}",
            expected_status=200,
            expected_cache="MISS",
            expected_length=body_bytes,
            headers={},
        )
        for request_id in range(count)
    ]


def hit_warmup(key_count: int, body_bytes: int) -> list[BenchRequest]:
    return [
        BenchRequest(
            path=f"/hit/{key_id}",
            expected_status=200,
            expected_cache="MISS",
            expected_length=body_bytes,
            headers={},
        )
        for key_id in range(key_count)
    ]


def hit_requests(requests: int, key_count: int, body_bytes: int) -> list[BenchRequest]:
    return [
        BenchRequest(
            path=f"/hit/{request_id % key_count}",
            expected_status=200,
            expected_cache="HIT",
            expected_length=body_bytes,
            headers={},
        )
        for request_id in range(requests)
    ]


def revalidate_warmup(body_bytes: int) -> list[BenchRequest]:
    return [
        BenchRequest(
            path="/revalidate",
            expected_status=200,
            expected_cache="MISS",
            expected_length=body_bytes,
            headers={},
        )
    ]


def revalidate_requests(requests: int, body_bytes: int) -> list[BenchRequest]:
    return [
        BenchRequest(
            path="/revalidate",
            expected_status=200,
            expected_cache="REVALIDATED",
            expected_length=body_bytes,
            headers={},
        )
        for _ in range(requests)
    ]


def slice_warmup() -> list[BenchRequest]:
    return [
        BenchRequest(
            path="/slice",
            expected_status=206,
            expected_cache="MISS",
            expected_length=3,
            headers={"Range": "bytes=2-4"},
        )
    ]


def slice_requests(requests: int) -> list[BenchRequest]:
    return [
        BenchRequest(
            path="/slice",
            expected_status=206,
            expected_cache="HIT",
            expected_length=2,
            headers={"Range": "bytes=5-6"},
        )
        for _ in range(requests)
    ]


def run_warmup(port: int, requests: list[BenchRequest], timeout: float) -> None:
    for request in requests:
        run_request(port, request, timeout)


def print_table(rows: list[ScenarioRow]) -> None:
    print(
        "| scenario | requests | concurrency | expected_cache | upstream_requests | elapsed_s | req_per_sec | avg_ms | p95_ms |"
    )
    print("| --- | ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: |")
    for row in rows:
        print(
            f"| {row.name} | {row.requests} | {row.concurrency} | {row.expected_cache} | "
            f"{row.upstream_requests} | {row.elapsed_s:.3f} | {row.req_per_sec:.2f} | "
            f"{row.avg_ms:.2f} | {row.p95_ms:.2f} |"
        )


def terminate_process(process: subprocess.Popen[str]) -> None:
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run cache-specific benchmark scenarios against a real rginx process."
    )
    parser.add_argument("--binary", help="Path to an existing rginx binary")
    parser.add_argument("--rebuild", action="store_true", help="Force cargo build --release -p rginx")
    parser.add_argument("--requests", type=int, default=400)
    parser.add_argument("--fill-keys", type=int, default=256)
    parser.add_argument("--hit-keys", type=int, default=64)
    parser.add_argument("--concurrency", type=int, default=32)
    parser.add_argument("--body-bytes", type=int, default=64 * 1024)
    parser.add_argument("--slice-size-bytes", type=int, default=8192)
    parser.add_argument("--slice-payload-bytes", type=int, default=32 * 1024)
    parser.add_argument("--timeout", type=float, default=5.0)
    parser.add_argument("--ready-timeout", type=float, default=30.0)
    args = parser.parse_args()

    if args.requests < 1 or args.fill_keys < 1 or args.hit_keys < 1 or args.concurrency < 1:
        parser.error("--requests, --fill-keys, --hit-keys, and --concurrency must be >= 1")
    if args.body_bytes < 1 or args.slice_size_bytes < 1 or args.slice_payload_bytes < 8:
        parser.error("--body-bytes, --slice-size-bytes, and --slice-payload-bytes must be positive")

    root = Path(__file__).resolve().parent.parent
    binary = ensure_rginx_binary(root, args.binary, args.rebuild)
    if not binary.exists():
        raise FileNotFoundError(f"rginx binary does not exist: {binary}")

    origin_state = OriginState(args.body_bytes, args.slice_payload_bytes)
    origin_server = ThreadingHttpServer(("127.0.0.1", 0), OriginHandler)
    origin_server.state = origin_state  # type: ignore[attr-defined]
    origin_port = int(origin_server.server_address[1])
    origin_thread = threading.Thread(target=origin_server.serve_forever, daemon=True)
    origin_thread.start()

    with tempfile.TemporaryDirectory(prefix="rginx-cache-bench-") as temp_dir:
        temp_root = Path(temp_dir)
        config_path = temp_root / "rginx.ron"
        log_path = temp_root / "rginx.log"
        cache_dir = temp_root / "cache"
        cache_dir.mkdir(parents=True, exist_ok=True)
        max_entry_bytes = max(args.body_bytes, args.slice_payload_bytes) + 4096
        cache_max_size_bytes = estimate_cache_max_size_bytes(
            max_entry_bytes,
            args.fill_keys,
            args.hit_keys,
        )
        process, listen_port = start_rginx(
            root,
            binary,
            config_path,
            log_path,
            origin_port,
            cache_dir,
            cache_max_size_bytes,
            max_entry_bytes,
            args.slice_size_bytes,
            args.ready_timeout,
        )
        try:
            rows: list[ScenarioRow] = []

            before_fill = origin_state.count("fill")
            fill = fill_requests(args.fill_keys, args.body_bytes)
            elapsed, req_per_sec, avg_ms, p95_ms = benchmark(
                listen_port,
                fill,
                min(args.concurrency, len(fill)),
                args.timeout,
            )
            rows.append(
                ScenarioRow(
                    name="fill",
                    requests=len(fill),
                    concurrency=min(args.concurrency, len(fill)),
                    expected_cache="MISS",
                    upstream_requests=origin_state.count("fill") - before_fill,
                    elapsed_s=elapsed,
                    req_per_sec=req_per_sec,
                    avg_ms=avg_ms,
                    p95_ms=p95_ms,
                )
            )

            before_hit = origin_state.count("hit")
            run_warmup(listen_port, hit_warmup(args.hit_keys, args.body_bytes), args.timeout)
            warmed_hit = origin_state.count("hit") - before_hit
            hit = hit_requests(args.requests, args.hit_keys, args.body_bytes)
            elapsed, req_per_sec, avg_ms, p95_ms = benchmark(
                listen_port,
                hit,
                min(args.concurrency, len(hit)),
                args.timeout,
            )
            rows.append(
                ScenarioRow(
                    name="hit",
                    requests=len(hit),
                    concurrency=min(args.concurrency, len(hit)),
                    expected_cache="HIT",
                    upstream_requests=origin_state.count("hit") - before_hit,
                    elapsed_s=elapsed,
                    req_per_sec=req_per_sec,
                    avg_ms=avg_ms,
                    p95_ms=p95_ms,
                )
            )
            if origin_state.count("hit") - before_hit != warmed_hit:
                raise RuntimeError("hit benchmark unexpectedly forwarded requests upstream")

            before_revalidate = origin_state.count("revalidate")
            run_warmup(listen_port, revalidate_warmup(args.body_bytes), args.timeout)
            revalidate = revalidate_requests(args.requests, args.body_bytes)
            elapsed, req_per_sec, avg_ms, p95_ms = benchmark(
                listen_port,
                revalidate,
                min(args.concurrency, len(revalidate)),
                args.timeout,
            )
            rows.append(
                ScenarioRow(
                    name="revalidate",
                    requests=len(revalidate),
                    concurrency=min(args.concurrency, len(revalidate)),
                    expected_cache="REVALIDATED",
                    upstream_requests=origin_state.count("revalidate") - before_revalidate,
                    elapsed_s=elapsed,
                    req_per_sec=req_per_sec,
                    avg_ms=avg_ms,
                    p95_ms=p95_ms,
                )
            )

            before_slice = origin_state.count("slice")
            run_warmup(listen_port, slice_warmup(), args.timeout)
            warmed_slice = origin_state.count("slice") - before_slice
            slice = slice_requests(args.requests)
            elapsed, req_per_sec, avg_ms, p95_ms = benchmark(
                listen_port,
                slice,
                min(args.concurrency, len(slice)),
                args.timeout,
            )
            rows.append(
                ScenarioRow(
                    name="slice_hit",
                    requests=len(slice),
                    concurrency=min(args.concurrency, len(slice)),
                    expected_cache="HIT",
                    upstream_requests=origin_state.count("slice") - before_slice,
                    elapsed_s=elapsed,
                    req_per_sec=req_per_sec,
                    avg_ms=avg_ms,
                    p95_ms=p95_ms,
                )
            )
            if origin_state.count("slice") - before_slice != warmed_slice:
                raise RuntimeError("slice-hit benchmark unexpectedly forwarded requests upstream")

            print_table(rows)
            return 0
        finally:
            terminate_process(process)
            origin_server.shutdown()
            origin_server.server_close()
            origin_thread.join(timeout=5)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        print("interrupted", file=sys.stderr)
        raise SystemExit(130)
