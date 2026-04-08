#!/usr/bin/env python3
from __future__ import annotations

import argparse
import base64
import concurrent.futures
import statistics
import subprocess
import sys
import time
from dataclasses import dataclass


@dataclass(frozen=True)
class Scenario:
    name: str
    url: str
    flags: list[str]
    headers: list[str]
    body: bytes | None = None


def grpc_frame(payload: bytes) -> bytes:
    return bytes([0]) + len(payload).to_bytes(4, byteorder="big") + payload


def run_curl(scenario: Scenario, timeout: float) -> float:
    started = time.perf_counter()
    command = [
        "curl",
        "--silent",
        "--show-error",
        "--fail",
        "--output",
        "/dev/null",
        "--max-time",
        str(timeout),
        *scenario.flags,
    ]
    for header in scenario.headers:
        command.extend(["--header", header])
    command.append(scenario.url)

    subprocess.run(
        command,
        input=scenario.body,
        check=True,
    )
    return time.perf_counter() - started


def benchmark(
    scenario: Scenario,
    requests: int,
    concurrency: int,
    timeout: float,
) -> dict[str, object]:
    started = time.perf_counter()
    durations: list[float] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as executor:
        futures = [executor.submit(run_curl, scenario, timeout) for _ in range(requests)]
        for future in concurrent.futures.as_completed(futures):
            durations.append(future.result())
    elapsed = time.perf_counter() - started
    req_per_sec = requests / elapsed if elapsed else 0.0
    avg_ms = statistics.mean(durations) * 1000 if durations else 0.0
    return {
        "scenario": scenario.name,
        "url": scenario.url,
        "requests": requests,
        "concurrency": concurrency,
        "elapsed_s": round(elapsed, 3),
        "req_per_sec": round(req_per_sec, 2),
        "avg_ms": round(avg_ms, 2),
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Run the benchmark matrix for HTTP, TLS, HTTP/2, gRPC, and grpc-web."
    )
    parser.add_argument("--http1-url")
    parser.add_argument("--https-url")
    parser.add_argument("--http2-url")
    parser.add_argument("--grpc-url")
    parser.add_argument("--grpc-web-url")
    parser.add_argument("--grpc-web-text-url")
    parser.add_argument(
        "--grpc-payload-hex",
        default="",
        help="Unary gRPC payload bytes encoded as hex; defaults to an empty protobuf message",
    )
    parser.add_argument("--requests", type=int, default=200)
    parser.add_argument("--concurrency", type=int, default=16)
    parser.add_argument("--timeout", type=float, default=5.0)
    args = parser.parse_args()

    try:
        grpc_payload = bytes.fromhex(args.grpc_payload_hex)
    except ValueError as error:
        parser.error(f"--grpc-payload-hex is not valid hex: {error}")
    grpc_body = grpc_frame(grpc_payload)

    scenarios: list[Scenario] = []
    if args.http1_url:
        scenarios.append(
            Scenario("http1_plain", args.http1_url, ["--http1.1"], [])
        )
    if args.https_url:
        scenarios.append(
            Scenario("https_tls", args.https_url, ["--http1.1", "--insecure"], [])
        )
    if args.http2_url:
        scenarios.append(
            Scenario("http2_tls", args.http2_url, ["--http2", "--insecure"], [])
        )
    if args.grpc_url:
        scenarios.append(
            Scenario(
                "grpc_h2",
                args.grpc_url,
                ["--http2", "--insecure", "--request", "POST", "--data-binary", "@-"],
                ["content-type: application/grpc", "te: trailers"],
                grpc_body,
            )
        )
    if args.grpc_web_url:
        scenarios.append(
            Scenario(
                "grpc_web_binary",
                args.grpc_web_url,
                ["--http1.1", "--request", "POST", "--data-binary", "@-"],
                ["content-type: application/grpc-web+proto", "x-grpc-web: 1"],
                grpc_body,
            )
        )
    if args.grpc_web_text_url:
        scenarios.append(
            Scenario(
                "grpc_web_text",
                args.grpc_web_text_url,
                ["--http1.1", "--request", "POST", "--data-binary", "@-"],
                ["content-type: application/grpc-web-text+proto", "x-grpc-web: 1"],
                base64.b64encode(grpc_body),
            )
        )

    if not scenarios:
        parser.error(
            "at least one benchmark URL must be provided; "
            "supported flags are --http1-url, --https-url, --http2-url, "
            "--grpc-url, --grpc-web-url, and --grpc-web-text-url"
        )

    rows = []
    for scenario in scenarios:
        rows.append(benchmark(scenario, args.requests, args.concurrency, args.timeout))

    print("| scenario | requests | concurrency | elapsed_s | req_per_sec | avg_ms |")
    print("| --- | ---: | ---: | ---: | ---: | ---: |")
    for row in rows:
        print(
            f"| {row['scenario']} | {row['requests']} | {row['concurrency']} | "
            f"{row['elapsed_s']} | {row['req_per_sec']} | {row['avg_ms']} |"
        )
    return 0


if __name__ == "__main__":
    sys.exit(main())
