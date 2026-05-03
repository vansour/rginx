#!/usr/bin/env python3
from __future__ import annotations

import argparse
import contextlib
import http.client
import json
import logging
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass
from pathlib import Path

READY_PATH = "/-/ready"
ORIGIN_PORT = 19000
RGINX_PORT = 18080
NGINX_PORT = 18081
COMPOSE_PROJECT = "rginx-cache-compare"


@dataclass(frozen=True)
class Profile:
    name: str
    rginx_worker_threads: int
    rginx_accept_workers: int
    nginx_worker_processes: int
    nginx_worker_connections: int
    wrk_threads: int
    wrk_connections: int


@dataclass(frozen=True)
class Scenario:
    name: str
    path: str
    probe_path: str
    probe_headers: dict[str, str]
    warmup: list[tuple[str, dict[str, str], str | None]]
    lua_script: str
    wrk_threads: int | None
    expected_cache_header: str | None
    expected_status: int
    duration_s: int
    upstream_counter: str
    notes: str


@dataclass
class ScenarioResult:
    target: str
    profile: str
    scenario: str
    requests: int
    req_per_sec: float
    avg_ms: float
    stdev_ms: float
    p50_ms: float
    p95_ms: float | None
    p99_ms: float
    transfer_mb_s: float
    upstream_requests: int
    mem_avg_mb: float
    mem_peak_mb: float
    ready_s: float


PROFILES = [
    Profile(
        name="single",
        rginx_worker_threads=1,
        rginx_accept_workers=1,
        nginx_worker_processes=1,
        nginx_worker_connections=4096,
        wrk_threads=2,
        wrk_connections=64,
    ),
    Profile(
        name="multi4",
        rginx_worker_threads=4,
        rginx_accept_workers=4,
        nginx_worker_processes=4,
        nginx_worker_connections=4096,
        wrk_threads=4,
        wrk_connections=256,
    ),
]


SCENARIOS = [
    Scenario(
        name="fill_unique",
        path="/fill/",
        probe_path="/fill/probe-0",
        probe_headers={},
        warmup=[],
        lua_script="fill_unique.lua",
        wrk_threads=1,
        expected_cache_header="MISS",
        expected_status=200,
        duration_s=5,
        upstream_counter="fill",
        notes="cold unique fills",
    ),
    Scenario(
        name="warm_hit",
        path="/hit/",
        probe_path="/hit/0",
        probe_headers={},
        warmup=[],
        lua_script="warm_hit.lua",
        wrk_threads=None,
        expected_cache_header="HIT",
        expected_status=200,
        duration_s=20,
        upstream_counter="hit",
        notes="steady-state cached hotset hits",
    ),
    Scenario(
        name="revalidate",
        path="/revalidate",
        probe_path="/revalidate",
        probe_headers={},
        warmup=[("/revalidate", {}, "MISS")],
        lua_script="revalidate.lua",
        wrk_threads=None,
        expected_cache_header="REVALIDATED",
        expected_status=200,
        duration_s=15,
        upstream_counter="revalidate",
        notes="conditional revalidation workload",
    ),
    Scenario(
        name="slice_hit",
        path="/slice",
        probe_path="/slice",
        probe_headers={"Range": "bytes=5-6"},
        warmup=[("/slice", {"Range": "bytes=2-4"}, "MISS")],
        lua_script="slice_hit.lua",
        wrk_threads=None,
        expected_cache_header="HIT",
        expected_status=206,
        duration_s=20,
        upstream_counter="slice",
        notes="range/slice steady-state hits",
    ),
]


def run(cmd: list[str], cwd: Path, env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, cwd=cwd, env=env, text=True, capture_output=True, check=True)


def docker_compose(project_dir: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return run(["docker", "compose", *args], cwd=project_dir)


def write_file(path: Path, contents: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(contents, encoding="utf-8")


def generate_origin_command(body_bytes: int, slice_payload_bytes: int, hot_fill_delay_ms: int) -> str:
    return (
        "python /workspace/perf/cache_compare/origin_server.py "
        f"--port 9000 --body-bytes {body_bytes} "
        f"--slice-payload-bytes {slice_payload_bytes} --hot-fill-delay-ms {hot_fill_delay_ms}"
    )


def rginx_config(cache_dir: str, upstream: str, profile: Profile, body_bytes: int, slice_size_bytes: int) -> str:
    max_entry_bytes = max(body_bytes, slice_size_bytes * 8) + 4096
    return f"""Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 2,
        worker_threads: Some({profile.rginx_worker_threads}),
        accept_workers: Some({profile.rginx_accept_workers}),
    ),
    cache_zones: [
        CacheZoneConfig(
            name: "default",
            path: "{cache_dir}",
            max_size_bytes: Some({max_entry_bytes * 512}),
            inactive_secs: Some(600),
            default_ttl_secs: Some(60),
            max_entry_bytes: Some({max_entry_bytes}),
        ),
    ],
    server: ServerConfig(
        listen: "0.0.0.0:8080",
    ),
    upstreams: [
        UpstreamConfig(
            name: "backend",
            peers: [
                UpstreamPeerConfig(
                    url: "{upstream}",
                ),
            ],
            request_timeout_secs: Some(5),
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
            matcher: Prefix("/fill"),
            handler: Proxy(upstream: "backend"),
            cache: Some(CacheRouteConfig(
                zone: "default",
                methods: Some(["GET", "HEAD"]),
                statuses: Some([200]),
                key: Some("{{scheme}}:{{uri}}"),
                stale_if_error_secs: Some(60),
            )),
        ),
        LocationConfig(
            matcher: Prefix("/hit"),
            handler: Proxy(upstream: "backend"),
            cache: Some(CacheRouteConfig(
                zone: "default",
                methods: Some(["GET", "HEAD"]),
                statuses: Some([200]),
                key: Some("{{scheme}}:{{uri}}"),
                stale_if_error_secs: Some(60),
            )),
        ),
        LocationConfig(
            matcher: Exact("/revalidate"),
            handler: Proxy(upstream: "backend"),
            cache: Some(CacheRouteConfig(
                zone: "default",
                methods: Some(["GET", "HEAD"]),
                statuses: Some([200]),
                key: Some("{{scheme}}:{{uri}}"),
                stale_if_error_secs: Some(60),
            )),
        ),
        LocationConfig(
            matcher: Exact("/slice"),
            handler: Proxy(upstream: "backend"),
            cache: Some(CacheRouteConfig(
                zone: "default",
                methods: Some(["GET", "HEAD"]),
                statuses: Some([206]),
                key: Some("{{scheme}}:{{uri}}"),
                stale_if_error_secs: Some(60),
                range_requests: Some(Cache),
                slice_size_bytes: Some({slice_size_bytes}),
            )),
        ),
    ],
)
"""


def nginx_config(cache_dir: str, profile: Profile, slice_size_bytes: int) -> str:
    return f"""worker_processes {profile.nginx_worker_processes};
pid /tmp/nginx.pid;
events {{
    worker_connections {profile.nginx_worker_connections};
}}
http {{
    access_log off;
    error_log /dev/stderr warn;
    sendfile on;
    tcp_nopush on;
    tcp_nodelay on;
    keepalive_timeout 30;
    types_hash_max_size 2048;

    proxy_cache_path {cache_dir} levels=1:2 keys_zone=cache_zone:16m inactive=600s max_size=512m use_temp_path=off;

    upstream backend {{
        server origin:9000;
        keepalive 64;
    }}

    map $upstream_cache_status $cache_header {{
        default $upstream_cache_status;
        "" "BYPASS";
    }}

    server {{
        listen 8080;
        server_name _;

        location = /-/ready {{
            return 200 "ready\\n";
        }}

        location /fill/ {{
            proxy_pass http://backend;
            proxy_http_version 1.1;
            proxy_set_header Connection "";
            proxy_cache cache_zone;
            proxy_cache_methods GET HEAD;
            proxy_cache_key "$scheme$proxy_host$request_uri";
            proxy_cache_valid 200 60s;
            proxy_cache_lock on;
            add_header X-Cache $cache_header always;
        }}

        location /hit/ {{
            proxy_pass http://backend;
            proxy_http_version 1.1;
            proxy_set_header Connection "";
            proxy_cache cache_zone;
            proxy_cache_methods GET HEAD;
            proxy_cache_key "$scheme$proxy_host$request_uri";
            proxy_cache_valid 200 60s;
            proxy_cache_lock on;
            add_header X-Cache $cache_header always;
        }}

        location = /revalidate {{
            proxy_pass http://backend;
            proxy_http_version 1.1;
            proxy_set_header Connection "";
            proxy_cache cache_zone;
            proxy_cache_methods GET HEAD;
            proxy_cache_key "$scheme$proxy_host$request_uri";
            proxy_cache_valid 200 60s;
            proxy_cache_revalidate on;
            proxy_cache_background_update off;
            proxy_cache_lock on;
            add_header X-Cache $cache_header always;
        }}

        location = /slice {{
            proxy_pass http://backend;
            proxy_http_version 1.1;
            proxy_set_header Connection "";
            proxy_cache cache_zone;
            proxy_cache_methods GET HEAD;
            proxy_cache_key "$scheme$proxy_host$request_uri$slice_range";
            proxy_cache_valid 200 206 60s;
            proxy_cache_lock on;
            slice {slice_size_bytes};
            proxy_set_header Range $slice_range;
            add_header X-Cache $cache_header always;
        }}
    }}
}}
"""


def compose_yaml(project_root: Path, work_dir: Path) -> str:
    return f"""services:
  origin:
    image: python:3.13-slim
    working_dir: /workspace
    command: >-
      {generate_origin_command(body_bytes=65536, slice_payload_bytes=32768, hot_fill_delay_ms=250)}
    volumes:
      - {project_root.as_posix()}:/workspace:ro
    ports:
      - "{ORIGIN_PORT}:9000"

  rginx:
    build:
      context: {project_root.as_posix()}
      dockerfile: perf/cache_compare/Dockerfile.rginx
    working_dir: /workspace
    command: ["--config", "/run/rginx/rginx.ron"]
    volumes:
      - {work_dir.as_posix()}/rginx/rginx.ron:/run/rginx/rginx.ron:ro
      - {work_dir.as_posix()}/rginx/cache:/cache
    ports:
      - "{RGINX_PORT}:8080"
    depends_on:
      - origin

  nginx:
    build:
      context: {project_root.as_posix()}
      dockerfile: perf/cache_compare/Dockerfile.nginx
    volumes:
      - {work_dir.as_posix()}/nginx/nginx.conf:/etc/nginx/nginx.conf:ro
      - {work_dir.as_posix()}/nginx/cache:/cache
    ports:
      - "{NGINX_PORT}:8080"
    depends_on:
      - origin

  wrk:
    build:
      context: {project_root.as_posix()}
      dockerfile: perf/cache_compare/Dockerfile.wrk
    working_dir: /wrk
    volumes:
      - {work_dir.as_posix()}/wrk:/wrk:ro
"""


def lua_fill_unique(fill_keys: int) -> str:
    return f"""counter = 0
request = function()
  local id = counter % {fill_keys}
  counter = counter + 1
  return wrk.format("GET", "/fill/" .. id)
end
"""


def lua_warm_hit(hit_keys: int) -> str:
    return f"""counter = 0
request = function()
  local id = counter % {hit_keys}
  counter = counter + 1
  return wrk.format("GET", "/hit/" .. id)
end
"""


def lua_revalidate() -> str:
    return """request = function()
  return wrk.format("GET", "/revalidate")
end
"""


def lua_slice() -> str:
    return """request = function()
  return wrk.format("GET", "/slice", {["Range"] = "bytes=5-6"})
end
"""


def http_get(port: int, path: str, headers: dict[str, str] | None = None, timeout: float = 5.0) -> tuple[int, dict[str, str], bytes]:
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    conn.request("GET", path, headers=headers or {})
    response = conn.getresponse()
    body = response.read()
    response_headers = {k.lower(): v for k, v in response.getheaders()}
    conn.close()
    return response.status, response_headers, body


def http_post(port: int, path: str, timeout: float = 5.0) -> tuple[int, bytes]:
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    conn.request("POST", path, body=b"", headers={"Content-Length": "0"})
    response = conn.getresponse()
    body = response.read()
    conn.close()
    return response.status, body


def wait_ready(port: int, timeout: float) -> float:
    started = time.perf_counter()
    deadline = started + timeout
    while time.perf_counter() < deadline:
        try:
            status, _headers, _body = http_get(port, READY_PATH, timeout=0.5)
            if status == 200:
                return time.perf_counter() - started
        except OSError:
            pass
        time.sleep(0.1)
    raise TimeoutError(f"timed out waiting for server on port {port}")


def origin_reset() -> None:
    status, _body = http_post(ORIGIN_PORT, "/-/reset")
    if status != 200:
        raise RuntimeError(f"failed to reset origin stats: {status}")


def origin_stats() -> dict[str, int]:
    status, _headers, body = http_get(ORIGIN_PORT, "/-/stats")
    if status != 200:
        raise RuntimeError(f"failed to fetch origin stats: {status}")
    return json.loads(body.decode())


def sample_memory(project_dir: Path, service: str, stop: threading.Event, samples: list[float]) -> None:
    while not stop.is_set():
        try:
            result = docker_compose(project_dir, "ps", "-q", service)
            container_id = result.stdout.strip()
            if container_id:
                stats = run(
                    [
                        "docker",
                        "stats",
                        "--no-stream",
                        "--format",
                        "{{.MemUsage}}",
                        container_id,
                    ],
                    cwd=project_dir,
                )
                samples.append(parse_mem_usage_mb(stats.stdout.strip()))
        except (OSError, subprocess.CalledProcessError, ValueError) as error:
            logging.debug("memory sampling failed for %s: %s", service, error)
        stop.wait(1.0)


def parse_mem_usage_mb(value: str) -> float:
    if not value:
        return 0.0
    current = value.split("/", 1)[0].strip()
    match = re.match(r"([0-9.]+)([KMG]iB)", current)
    if not match:
        return 0.0
    number = float(match.group(1))
    unit = match.group(2)
    if unit == "KiB":
        return number / 1024
    if unit == "MiB":
        return number
    if unit == "GiB":
        return number * 1024
    return 0.0


def parse_wrk_output(output: str) -> dict[str, float | None]:
    requests = int(re.search(r"(\d+) requests in", output).group(1))
    req_per_sec = float(re.search(r"Requests/sec:\s+([0-9.]+)", output).group(1))
    transfer_match = re.search(r"Transfer/sec:\s+([0-9.]+)([KMG]?B)", output)
    transfer_mb_s = convert_transfer_to_mb_s(float(transfer_match.group(1)), transfer_match.group(2))
    latency_avg = parse_time_ms(re.search(r"Latency\s+([0-9.]+[a-z]+)\s+", output).group(1))
    latency_stdev = parse_time_ms(re.search(r"Latency\s+[0-9.]+[a-z]+\s+([0-9.]+[a-z]+)\s+", output).group(1))
    p50 = parse_percentile_ms(output, "50")
    p95 = parse_percentile_ms(output, "95")
    p99 = parse_percentile_ms(output, "99")
    return {
        "requests": requests,
        "req_per_sec": req_per_sec,
        "avg_ms": latency_avg,
        "stdev_ms": latency_stdev,
        "p50_ms": p50,
        "p95_ms": p95,
        "p99_ms": p99,
        "transfer_mb_s": transfer_mb_s,
    }


def convert_transfer_to_mb_s(value: float, unit: str) -> float:
    if unit == "B":
        return value / (1024 * 1024)
    if unit == "KB":
        return value / 1024
    if unit == "MB":
        return value
    if unit == "GB":
        return value * 1024
    raise ValueError(f"unsupported transfer unit: {unit}")


def parse_percentile_ms(output: str, percentile: str) -> float | None:
    match = re.search(rf"\s+{percentile}(?:\.0+)?%\s+([0-9.]+[a-z]+)", output)
    return parse_time_ms(match.group(1)) if match else None


def parse_time_ms(value: str) -> float:
    match = re.match(r"([0-9.]+)(us|ms|s)", value)
    if not match:
        raise ValueError(f"unsupported time value: {value}")
    number = float(match.group(1))
    unit = match.group(2)
    if unit == "us":
        return number / 1000
    if unit == "ms":
        return number
    if unit == "s":
        return number * 1000
    raise ValueError(f"unsupported unit: {unit}")


def request_host_header(target: str) -> str:
    return target


def warmup_target(port: int, target: str, scenario: Scenario, hit_keys: int) -> None:
    if scenario.name == "warm_hit":
        for key_id in range(hit_keys):
            status, headers, _body = http_get(port, f"/hit/{key_id}", headers={"Host": request_host_header(target)})
            if status != 200:
                raise RuntimeError(f"warm_hit warmup failed with status {status}")
            x_cache = headers.get("x-cache")
            if x_cache not in {"MISS", "EXPIRED"}:
                raise RuntimeError(f"warm_hit warmup returned unexpected x-cache={x_cache!r}")
        for key_id in range(hit_keys):
            status, headers, _body = http_get(port, f"/hit/{key_id}", headers={"Host": request_host_header(target)})
            if status != 200:
                raise RuntimeError(f"warm_hit verification failed with status {status}")
            if headers.get("x-cache") != "HIT":
                raise RuntimeError(f"warm_hit verification expected x-cache='HIT', got {headers.get('x-cache')!r}")
        return

    for path, headers, expected_cache in scenario.warmup:
        merged_headers = {"Host": request_host_header(target), **headers}
        status, response_headers, _body = http_get(port, path, headers=merged_headers)
        if status not in {200, 206}:
            raise RuntimeError(f"{scenario.name} warmup failed with status {status}")
        if expected_cache is not None and response_headers.get("x-cache") != expected_cache:
            raise RuntimeError(
                f"{scenario.name} warmup expected x-cache={expected_cache!r}, "
                f"got {response_headers.get('x-cache')!r}"
            )


def verify_target_response(port: int, target: str, scenario: Scenario) -> None:
    headers = {"Host": request_host_header(target), **scenario.probe_headers}
    status, response_headers, _body = http_get(port, scenario.probe_path, headers=headers)
    if status != scenario.expected_status:
        raise RuntimeError(
            f"{scenario.name} probe expected status {scenario.expected_status}, got {status}"
        )
    if scenario.expected_cache_header is not None and response_headers.get("x-cache") != scenario.expected_cache_header:
        raise RuntimeError(
            f"{scenario.name} probe expected x-cache={scenario.expected_cache_header!r}, "
            f"got {response_headers.get('x-cache')!r}"
        )


def write_wrk_scripts(work_dir: Path, fill_keys: int, hit_keys: int) -> None:
    write_file(work_dir / "wrk" / "fill_unique.lua", lua_fill_unique(fill_keys))
    write_file(work_dir / "wrk" / "warm_hit.lua", lua_warm_hit(hit_keys))
    write_file(work_dir / "wrk" / "revalidate.lua", lua_revalidate())
    write_file(work_dir / "wrk" / "slice_hit.lua", lua_slice())


def prepare_workspace(project_root: Path, temp_root: Path, profile: Profile, body_bytes: int, slice_size_bytes: int) -> Path:
    work_dir = temp_root / profile.name
    shutil.rmtree(work_dir, ignore_errors=True)
    (work_dir / "rginx" / "cache").mkdir(parents=True, exist_ok=True)
    (work_dir / "nginx" / "cache").mkdir(parents=True, exist_ok=True)
    (work_dir / "wrk").mkdir(parents=True, exist_ok=True)
    (work_dir / "rginx" / "cache").chmod(0o777)
    (work_dir / "nginx" / "cache").chmod(0o777)
    write_file(
        work_dir / "compose.yaml",
        compose_yaml(project_root, work_dir),
    )
    write_file(
        work_dir / "rginx" / "rginx.ron",
        rginx_config("/cache", "http://origin:9000", profile, body_bytes, slice_size_bytes),
    )
    write_file(
        work_dir / "nginx" / "nginx.conf",
        nginx_config("/cache", profile, slice_size_bytes),
    )
    return work_dir


def build_images(project_dir: Path) -> None:
    docker_compose(project_dir, "build", "rginx", "nginx", "wrk")


def recreate_service(project_dir: Path, service: str) -> float:
    docker_compose(project_dir, "rm", "-sf", "rginx", "nginx")
    docker_compose(project_dir, "up", "-d", "--no-deps", service)
    port = RGINX_PORT if service == "rginx" else NGINX_PORT
    return wait_ready(port, timeout=120)


def ensure_origin(project_dir: Path) -> None:
    docker_compose(project_dir, "up", "-d", "origin")
    wait_ready(ORIGIN_PORT, timeout=30)


def down(project_dir: Path) -> None:
    with contextlib.suppress(subprocess.CalledProcessError):
        docker_compose(project_dir, "down", "--remove-orphans", "--volumes")


def run_wrk(project_dir: Path, target: str, profile: Profile, scenario: Scenario, timeout_s: int) -> str:
    url = f"http://{target}:8080{scenario.path}"
    wrk_threads = scenario.wrk_threads if scenario.wrk_threads is not None else profile.wrk_threads
    command = [
        "run",
        "--rm",
        "wrk",
        "--latency",
        "-t",
        str(wrk_threads),
        "-c",
        str(profile.wrk_connections),
        "-d",
        f"{scenario.duration_s}s",
        "--timeout",
        f"{timeout_s}s",
        "-s",
        f"/wrk/{scenario.lua_script}",
        url,
    ]
    result = docker_compose(project_dir, *command)
    return result.stdout


def scenario_port(target: str) -> int:
    return RGINX_PORT if target == "rginx" else NGINX_PORT


def reset_target_cache(project_dir: Path, target: str) -> None:
    cache_dir = project_dir / target / "cache"
    shutil.rmtree(cache_dir, ignore_errors=True)
    cache_dir.mkdir(parents=True, exist_ok=True)


def run_target_scenario(project_dir: Path, target: str, profile: Profile, scenario: Scenario, hit_keys: int, timeout_s: int) -> ScenarioResult:
    origin_reset()
    reset_target_cache(project_dir, target)
    ready_s = recreate_service(project_dir, target)
    port = scenario_port(target)
    warmup_target(port, target, scenario, hit_keys)
    verify_target_response(port, target, scenario)
    before_stats = origin_stats()

    stop = threading.Event()
    mem_samples: list[float] = []
    sampler = threading.Thread(
        target=sample_memory,
        args=(project_dir, target, stop, mem_samples),
        daemon=True,
    )
    sampler.start()
    try:
        output = run_wrk(project_dir, target, profile, scenario, timeout_s)
    finally:
        stop.set()
        sampler.join(timeout=3)

    after_stats = origin_stats()
    parsed = parse_wrk_output(output)
    upstream_requests = after_stats.get(scenario.upstream_counter, 0) - before_stats.get(scenario.upstream_counter, 0)
    mem_avg = statistics.mean(mem_samples) if mem_samples else 0.0
    mem_peak = max(mem_samples) if mem_samples else 0.0
    return ScenarioResult(
        target=target,
        profile=profile.name,
        scenario=scenario.name,
        requests=int(parsed["requests"]),
        req_per_sec=parsed["req_per_sec"],
        avg_ms=parsed["avg_ms"],
        stdev_ms=parsed["stdev_ms"],
        p50_ms=parsed["p50_ms"],
        p95_ms=parsed["p95_ms"],
        p99_ms=parsed["p99_ms"],
        transfer_mb_s=parsed["transfer_mb_s"],
        upstream_requests=upstream_requests,
        mem_avg_mb=mem_avg,
        mem_peak_mb=mem_peak,
        ready_s=ready_s,
    )


def print_results(results: list[ScenarioResult]) -> None:
    print("| target | profile | scenario | requests | req/s | avg_ms | p50_ms | p95_ms | p99_ms | upstream | mem_avg_mb | mem_peak_mb | ready_s |")
    print("| --- | --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |")
    for row in results:
        print(
            f"| {row.target} | {row.profile} | {row.scenario} | {row.requests} | "
            f"{row.req_per_sec:.2f} | {row.avg_ms:.2f} | {row.p50_ms:.2f} | "
            f"{format_ms(row.p95_ms)} | {row.p99_ms:.2f} | {row.upstream_requests} | "
            f"{row.mem_avg_mb:.2f} | {row.mem_peak_mb:.2f} | {row.ready_s:.3f} |"
        )


def format_ms(value: float | None) -> str:
    return "n/a" if value is None else f"{value:.2f}"


def main() -> int:
    parser = argparse.ArgumentParser(description="Compare rginx and nginx cache performance in isolated Docker containers.")
    parser.add_argument("--body-bytes", type=int, default=64 * 1024)
    parser.add_argument("--slice-size-bytes", type=int, default=8192)
    parser.add_argument("--fill-keys", type=int, default=1_000_000)
    parser.add_argument("--hit-keys", type=int, default=128)
    parser.add_argument("--timeout-secs", type=int, default=10)
    parser.add_argument("--profiles", nargs="*", choices=[profile.name for profile in PROFILES])
    args = parser.parse_args()

    selected_profiles = [profile for profile in PROFILES if not args.profiles or profile.name in args.profiles]
    project_root = Path(__file__).resolve().parent.parent

    all_results: list[ScenarioResult] = []
    with tempfile.TemporaryDirectory(prefix="rginx-cache-compare-") as temp_dir:
        temp_root = Path(temp_dir)
        for profile in selected_profiles:
            work_dir = prepare_workspace(project_root, temp_root, profile, args.body_bytes, args.slice_size_bytes)
            write_wrk_scripts(work_dir, args.fill_keys, args.hit_keys)
            down(work_dir)
            build_images(work_dir)
            ensure_origin(work_dir)
            for target in ("rginx", "nginx"):
                for scenario in SCENARIOS:
                    all_results.append(
                        run_target_scenario(
                            work_dir,
                            target,
                            profile,
                            scenario,
                            args.hit_keys,
                            args.timeout_secs,
                        )
                    )
            down(work_dir)

    print_results(all_results)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except KeyboardInterrupt:
        print("interrupted", file=sys.stderr)
