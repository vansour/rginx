#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

IMAGE_TAG="rginx-nginx-compare:trixie"
OUT_DIR="${ROOT_DIR}/target/nginx-compare"
BUILD_IMAGE=1

usage() {
    cat <<'EOF'
Usage: run-nginx-compare-docker.sh [options] [-- benchmark-args...]

Build and run the nginx vs rginx comparison inside a Debian trixie container.

Options:
  --image-tag <tag>
      Docker image tag, default: rginx-nginx-compare:trixie
  --out-dir <dir>
      Host output directory, default: target/nginx-compare
  --no-build
      Reuse an existing image instead of rebuilding it
  -h, --help
      Show help

Any additional arguments after `--` are passed through to scripts/nginx_compare.py.
EOF
}

log() {
    printf '[nginx-compare-docker] %s\n' "$*"
}

die() {
    printf '[nginx-compare-docker] error: %s\n' "$*" >&2
    exit 1
}

have() {
    command -v "$1" >/dev/null 2>&1
}

if ! have docker; then
    die "docker is required"
fi

extra_args=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --image-tag)
            [[ $# -ge 2 ]] || die "--image-tag requires a value"
            IMAGE_TAG="$2"
            shift 2
            ;;
        --out-dir)
            [[ $# -ge 2 ]] || die "--out-dir requires a value"
            OUT_DIR="$2"
            shift 2
            ;;
        --no-build)
            BUILD_IMAGE=0
            shift
            ;;
        --)
            shift
            extra_args=("$@")
            break
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            extra_args+=("$1")
            shift
            ;;
    esac
done

OUT_DIR="$(
    readlink -f "${OUT_DIR}" 2>/dev/null \
        || realpath "${OUT_DIR}" 2>/dev/null \
        || python3 -c 'import os,sys; print(os.path.abspath(sys.argv[1]))' "${OUT_DIR}"
)"
mkdir -p "${OUT_DIR}"

if [[ "${BUILD_IMAGE}" -eq 1 ]]; then
    log "building Docker image ${IMAGE_TAG}"
    docker build -t "${IMAGE_TAG}" -f "${ROOT_DIR}/docker/nginx-compare/Dockerfile" "${ROOT_DIR}"
fi

log "running comparison in Docker"
docker run --rm \
    -v "${OUT_DIR}:/out" \
    "${IMAGE_TAG}" \
    "${extra_args[@]}"

log "outputs written to ${OUT_DIR}"
