from __future__ import annotations

import argparse
import pathlib

from checkout import DEFAULT_NGINX_REF
from scenarios import run_comparison


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Compare rginx and nginx inside a reproducible local harness."
    )
    parser.add_argument("--workspace", type=pathlib.Path, required=True)
    parser.add_argument("--out-dir", type=pathlib.Path, required=True)
    parser.add_argument("--requests", type=int, default=20000)
    parser.add_argument("--concurrency", type=int, default=64)
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--nginx-ref", default=DEFAULT_NGINX_REF)
    parser.add_argument("--nginx-src-dir", type=pathlib.Path, default=None)
    parser.add_argument("--nginx-install-dir", type=pathlib.Path, default=None)
    args = parser.parse_args()

    return run_comparison(
        workspace=args.workspace.resolve(),
        out_dir=args.out_dir.resolve(),
        requests=args.requests,
        concurrency=args.concurrency,
        rounds=args.rounds,
        nginx_ref=args.nginx_ref,
        nginx_src_dir=args.nginx_src_dir,
        nginx_install_dir=args.nginx_install_dir,
    )
