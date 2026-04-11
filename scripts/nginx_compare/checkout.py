from __future__ import annotations

import pathlib

from common import run


NGINX_REPO_URL = "https://github.com/nginx/nginx"
DEFAULT_NGINX_REF = "release-1.29.8"


def ensure_nginx_checkout(src_dir: pathlib.Path, ref: str) -> str:
    if not src_dir.exists():
        run(["git", "clone", "--filter=blob:none", NGINX_REPO_URL, str(src_dir)])
    run(["git", "fetch", "--tags", "--force", "origin"], cwd=src_dir)
    run(["git", "checkout", "--detach", ref], cwd=src_dir)
    commit = run(["git", "rev-parse", "HEAD"], cwd=src_dir, capture_output=True)
    return commit.stdout.strip()


def current_git_head(path: pathlib.Path) -> str:
    try:
        completed = run(["git", "rev-parse", "HEAD"], cwd=path, capture_output=True)
    except RuntimeError:
        return "workspace-without-git-metadata"
    return completed.stdout.strip()
