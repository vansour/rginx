from __future__ import annotations

import base64
import dataclasses
import json
import os
import pathlib
import shutil
import tempfile

from build import ensure_nginx_binary, ensure_rginx_binary, nginx_version, rginx_version
from checkout import ensure_nginx_checkout
from common import UnsupportedScenario, reserve_port, write_text
from configs import (
    grpc_frame,
    nginx_grpc_proxy_config,
    nginx_proxy_config,
    nginx_reload_config,
    nginx_return_config,
    nginx_tls_return_config,
    rginx_grpc_proxy_config,
    rginx_proxy_config,
    rginx_reload_config,
    rginx_return_config,
    rginx_tls_return_config,
)
from launch import (
    generate_self_signed_cert,
    measure_reload_apply_time,
    run_single_server_benchmark,
    run_single_server_curl_benchmark,
    start_grpc_backend_server,
    start_upstream_server,
)
from render import render_markdown


def run_comparison(
    *,
    workspace: pathlib.Path,
    out_dir: pathlib.Path,
    requests: int,
    concurrency: int,
    nginx_ref: str,
    nginx_src_dir: pathlib.Path | None,
    nginx_install_dir: pathlib.Path | None,
) -> int:
    out_dir.mkdir(parents=True, exist_ok=True)

    build_root = pathlib.Path(tempfile.mkdtemp(prefix="rginx-nginx-compare-"))
    try:
        resolved_nginx_src_dir = (
            nginx_src_dir.resolve() if nginx_src_dir is not None else build_root / "nginx-src"
        )
        resolved_nginx_install_dir = (
            nginx_install_dir.resolve()
            if nginx_install_dir is not None
            else build_root / "nginx-install"
        )

        rginx_bin = ensure_rginx_binary(workspace)
        nginx_commit = ensure_nginx_checkout(resolved_nginx_src_dir, nginx_ref)
        nginx_bin = ensure_nginx_binary(
            resolved_nginx_src_dir,
            resolved_nginx_install_dir,
            nginx_ref=nginx_ref,
            nginx_commit=nginx_commit,
        )

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
            int(grpc_backend_port),
            cert_path=backend_cert,
            key_path=backend_key,
        )
        grpc_backend_port.release()
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

            write_text(rginx_return_path, rginx_return_config(int(port_plan["return"]["rginx"])))
            write_text(
                rginx_proxy_path,
                rginx_proxy_config(int(port_plan["proxy"]["rginx"]), upstream_port),
            )
            write_text(
                rginx_tls_return_path,
                rginx_tls_return_config(
                    int(port_plan["https_return"]["rginx"]),
                    frontend_cert,
                    frontend_key,
                ),
            )
            write_text(
                rginx_grpc_path,
                rginx_grpc_proxy_config(
                    int(port_plan["grpc"]["rginx"]),
                    frontend_cert,
                    frontend_key,
                    int(grpc_backend_port),
                ),
            )
            write_text(
                rginx_reload_path,
                rginx_reload_config(int(port_plan["reload"]["rginx"]), "old\\n"),
            )
            write_text(
                nginx_return_dir / "nginx.conf",
                nginx_return_config(int(port_plan["return"]["nginx"])),
            )
            write_text(
                nginx_proxy_dir / "nginx.conf",
                nginx_proxy_config(int(port_plan["proxy"]["nginx"]), upstream_port),
            )
            write_text(
                nginx_tls_return_dir / "nginx.conf",
                nginx_tls_return_config(
                    int(port_plan["https_return"]["nginx"]),
                    frontend_cert,
                    frontend_key,
                ),
            )
            write_text(
                nginx_grpc_dir / "nginx.conf",
                nginx_grpc_proxy_config(
                    int(port_plan["grpc"]["nginx"]),
                    frontend_cert,
                    frontend_key,
                    int(grpc_backend_port),
                ),
            )
            write_text(
                nginx_reload_dir / "nginx.conf",
                nginx_reload_config(int(port_plan["reload"]["nginx"]), "old\\n"),
            )

            results = []
            unsupported = []
            reload_results = []

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
                    requests=requests,
                    concurrency=concurrency,
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
                    requests=requests,
                    concurrency=concurrency,
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
                    requests=requests,
                    concurrency=concurrency,
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
                    requests=requests,
                    concurrency=concurrency,
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
                    requests=requests,
                    concurrency=concurrency,
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
                    requests=requests,
                    concurrency=concurrency,
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
                    requests=requests,
                    concurrency=concurrency,
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
                    requests=requests,
                    concurrency=concurrency,
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
                    requests=requests,
                    concurrency=concurrency,
                    grpc_mode="grpc",
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
                    requests=requests,
                    concurrency=concurrency,
                    grpc_mode="grpc",
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
                    requests=requests,
                    concurrency=concurrency,
                    grpc_mode="grpc-web-binary",
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
                    requests=requests,
                    concurrency=concurrency,
                    grpc_mode="grpc-web-text",
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
                    reloaded_config=rginx_reload_config(int(port_plan["reload"]["rginx"]), "new\\n"),
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
                    reloaded_config=nginx_reload_config(int(port_plan["reload"]["nginx"]), "new\\n"),
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
            "requests": requests,
            "concurrency": concurrency,
            "rginx_version": rginx_version_text,
            "nginx_version": nginx_version_text,
            "nginx_ref": nginx_ref,
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
                requests=requests,
                concurrency=concurrency,
                rginx_version_text=rginx_version_text,
                nginx_version_text=nginx_version_text,
                nginx_ref=nginx_ref,
                nginx_commit=nginx_commit,
            ),
        )
        print((out_dir / "performance-results.md").read_text(encoding="utf-8"))
        return 0
    finally:
        shutil.rmtree(build_root, ignore_errors=True)
