from __future__ import annotations

import dataclasses
import json
import os
import pathlib
import shutil
import subprocess

from checkout import current_git_head
from common import run, write_text


@dataclasses.dataclass(frozen=True)
class BuildStamp:
    values: dict[str, str]


def ensure_rginx_binary(workspace: pathlib.Path) -> pathlib.Path:
    binary = workspace / "target" / "release" / "rginx"
    stamp_path = binary.with_name(f"{binary.name}.benchmark-stamp.json")
    expected_stamp = BuildStamp(values={"workspace_head": current_git_head(workspace)})
    if binary.exists() and read_build_stamp(stamp_path) == expected_stamp:
        return binary
    run(["cargo", "build", "--release", "-p", "rginx", "--locked"], cwd=workspace)
    if not binary.exists():
        raise RuntimeError(f"rginx binary was not produced at {binary}")
    write_build_stamp(stamp_path, expected_stamp)
    return binary


def ensure_nginx_binary(
    src_dir: pathlib.Path,
    install_dir: pathlib.Path,
    *,
    nginx_ref: str,
    nginx_commit: str,
) -> pathlib.Path:
    binary = install_dir / "sbin" / "nginx"
    stamp_path = install_dir / ".benchmark-stamp.json"
    expected_stamp = BuildStamp(values={"nginx_ref": nginx_ref, "nginx_commit": nginx_commit})
    if binary.exists() and read_build_stamp(stamp_path) == expected_stamp:
        return binary

    if install_dir.exists():
        shutil.rmtree(install_dir)
    if (src_dir / "Makefile").exists():
        subprocess.run(
            ["make", "clean"],
            cwd=src_dir,
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

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
    write_build_stamp(stamp_path, expected_stamp)
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


def read_build_stamp(path: pathlib.Path) -> BuildStamp | None:
    if not path.exists():
        return None
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None
    if not isinstance(payload, dict) or not all(
        isinstance(key, str) and isinstance(value, str) for key, value in payload.items()
    ):
        return None
    return BuildStamp(values=payload)


def write_build_stamp(path: pathlib.Path, stamp: BuildStamp) -> None:
    write_text(path, json.dumps(stamp.values, sort_keys=True, indent=2) + "\n")
