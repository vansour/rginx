#!/usr/bin/env python3
from __future__ import annotations

import argparse
import concurrent.futures
import statistics
import subprocess
import sys
import time


def run_curl(url: str, extra_flags: list[str], timeout: float) -> float:
    started = time.perf_counter()
    subprocess.run(
        [
            "curl",
            "--silent",
            "--show-error",
            "--fail",
            "--output",
            "/dev/null",
            "--max-time",
            str(timeout),
            *extra_flags,
            url,
        ],
        check=True,
    )
    return time.perf_counter() - started


def benchmark(name: str, url: str, requests: int, concurrency: int, timeout: float, flags: list[str]) -> dict[str, object]:
    started = time.perf_counter()
    durations: list[float] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as executor:
        futures = [executor.submit(run_curl, url, flags, timeout) for _ in range(requests)]
        for future in concurrent.futures.as_completed(futures):
            durations.append(future.result())
    elapsed = time.perf_counter() - started
    req_per_sec = requests / elapsed if elapsed else 0.0
    avg_ms = statistics.mean(durations) * 1000 if durations else 0.0
    return {
        "scenario": name,
        "url": url,
        "requests": requests,
        "concurrency": concurrency,
        "elapsed_s": round(elapsed, 3),
        "req_per_sec": round(req_per_sec, 2),
        "avg_ms": round(avg_ms, 2),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Run the Week 8 benchmark matrix with curl.")
    parser.add_argument("--http1-url")
    parser.add_argument("--https-url")
    parser.add_argument("--http2-url")
    parser.add_argument("--requests", type=int, default=200)
    parser.add_argument("--concurrency", type=int, default=16)
    parser.add_argument("--timeout", type=float, default=5.0)
    args = parser.parse_args()

    scenarios: list[tuple[str, str, list[str]]] = []
    if args.http1_url:
        scenarios.append(("http1_plain", args.http1_url, ["--http1.1"]))
    if args.https_url:
        scenarios.append(("https_tls", args.https_url, ["--http1.1", "--insecure"]))
    if args.http2_url:
        scenarios.append(("http2_tls", args.http2_url, ["--http2", "--insecure"]))

    if not scenarios:
        parser.error("at least one of --http1-url, --https-url, or --http2-url must be provided")

    rows = []
    for name, url, flags in scenarios:
        rows.append(benchmark(name, url, args.requests, args.concurrency, args.timeout, flags))

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
