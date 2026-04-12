from __future__ import annotations

from common import BenchmarkResult, ReloadResult, UnsupportedScenario


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
    rounds: int,
    rginx_version_text: str,
    nginx_version_text: str,
    nginx_ref: str,
    nginx_commit: str,
) -> str:
    lines = [
        "# rginx vs nginx performance snapshot",
        "",
        "- Environment: Docker / Debian trixie",
        f"- Benchmark tools: Python HTTP/1.1 keepalive runner for {requests} requests / concurrency {concurrency}, `curl` threadpool for TLS / HTTP2 / HTTP3 / gRPC / grpc-web",
        f"- Reported values: median of {rounds} rounds",
        f"- rginx: `{rginx_version_text}`",
        f"- nginx: `{nginx_version_text}` (ref `{nginx_ref}`, source commit `{nginx_commit}`)",
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
            "- Reported throughput, latency, and reload numbers are medians across repeated rounds rather than a single sample.",
            "- HTTP/1.1 direct/proxy scenarios use a built-in Python keepalive client; TLS, HTTP/2, HTTP/3, gRPC, and grpc-web scenarios use concurrent `curl` processes.",
            "- HTTP/3 is currently benchmarked only for `rginx`; the nginx comparison build in this harness does not include QUIC/HTTP/3 support.",
            "- `grpc-web` is currently benchmarked only for `rginx`; `NGINX OSS` is recorded as unsupported because it has no native grpc-web translation path.",
            "- Add larger samples, repeated runs, TLS upstream verification variants, RSS/CPU sampling, and reload drain behavior before turning these numbers into external claims.",
        ]
    )
    return "\n".join(lines) + "\n"
