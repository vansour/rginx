#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
DEFAULT_PREFIX="$(cd "${SCRIPT_DIR}/.." && pwd)"

PREFIX="${DEFAULT_PREFIX}"
CONFIG_DIR=""
PURGE_CONFIG=0
YES=0
SUDO=""

usage() {
    cat <<'EOF'
Usage: uninstall.sh [options]

Options:
  --prefix <path>
      安装前缀，默认从脚本所在位置推断
  --config-dir <path>
      活跃配置目录，默认读取安装 manifest 或使用 <prefix>/etc/rginx
  --purge-config
      一并删除活跃配置目录
  -y, --yes
      不提示确认，直接执行
  -h, --help
      显示帮助
EOF
}

log() {
    printf '[uninstall] %s\n' "$*"
}

die() {
    printf '[uninstall] error: %s\n' "$*" >&2
    exit 1
}

have() {
    command -v "$1" >/dev/null 2>&1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --prefix)
            [[ $# -ge 2 ]] || die "--prefix requires a value"
            PREFIX="$2"
            shift 2
            ;;
        --config-dir)
            [[ $# -ge 2 ]] || die "--config-dir requires a value"
            CONFIG_DIR="$2"
            shift 2
            ;;
        --purge-config)
            PURGE_CONFIG=1
            shift
            ;;
        -y|--yes)
            YES=1
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown option: $1"
            ;;
    esac
done

nearest_existing_parent() {
    local path="$1"

    while [[ ! -e "${path}" ]]; do
        path="$(dirname "${path}")"
    done

    printf '%s\n' "${path}"
}

prepare_privileges() {
    local target parent

    if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
        return
    fi

    for target in "${BIN_DIR}" "${CONFIG_DIR}" "${DOC_DIR}" "${SHARE_DIR}"; do
        parent="$(nearest_existing_parent "${target}")"
        if [[ ! -w "${parent}" ]]; then
            have sudo || die "uninstall target requires elevated privileges, but sudo is not available"
            SUDO="sudo"
            return
        fi
    done
}

run_root() {
    if [[ -n "${SUDO}" ]]; then
        "${SUDO}" "$@"
        return
    fi

    "$@"
}

BIN_DIR="${PREFIX}/bin"
SHARE_DIR="${PREFIX}/share/rginx"
DOC_DIR="${PREFIX}/share/doc/rginx"
EXAMPLES_DIR="${SHARE_DIR}/configs"
MANIFEST_PATH="${SHARE_DIR}/install-manifest.txt"

if [[ -f "${MANIFEST_PATH}" ]]; then
    # shellcheck disable=SC1090
    source "${MANIFEST_PATH}"
fi

PREFIX="${prefix:-${PREFIX}}"
CONFIG_DIR="${CONFIG_DIR:-${config_dir:-${PREFIX}/etc/rginx}}"
BIN_DIR="${bin_dir:-${PREFIX}/bin}"
SHARE_DIR="${share_dir:-${PREFIX}/share/rginx}"
DOC_DIR="${doc_dir:-${PREFIX}/share/doc/rginx}"
EXAMPLES_DIR="${examples_dir:-${SHARE_DIR}/configs}"
ACTIVE_CONFIG="${active_config:-${CONFIG_DIR}/rginx.ron}"
MANIFEST_PATH="${SHARE_DIR}/install-manifest.txt"

prepare_privileges

if [[ "${YES}" -ne 1 ]]; then
    printf 'This will remove:\n'
    printf '  - %s\n' "${BIN_DIR}/rginx"
    printf '  - %s\n' "${BIN_DIR}/rginx-uninstall"
    printf '  - %s\n' "${DOC_DIR}"
    printf '  - %s\n' "${EXAMPLES_DIR}"
    if [[ "${PURGE_CONFIG}" -eq 1 ]]; then
        printf '  - %s\n' "${CONFIG_DIR}"
    else
        printf '  - preserve %s\n' "${CONFIG_DIR}"
    fi
    printf 'Continue? [y/N] '
    read -r answer
    case "${answer}" in
        y|Y|yes|YES)
            ;;
        *)
            die "aborted"
            ;;
    esac
fi

run_root rm -f "${BIN_DIR}/rginx"
run_root rm -f "${BIN_DIR}/rginx-uninstall"
run_root rm -rf "${DOC_DIR}"
run_root rm -rf "${EXAMPLES_DIR}"
run_root rm -f "${MANIFEST_PATH}"

if [[ "${PURGE_CONFIG}" -eq 1 ]]; then
    run_root rm -rf "${CONFIG_DIR}"
    run_root rmdir "$(dirname "${CONFIG_DIR}")" 2>/dev/null || true
    log "removed config dir: ${CONFIG_DIR}"
else
    log "preserved config dir: ${CONFIG_DIR}"
fi

run_root rmdir "$(dirname "${DOC_DIR}")" 2>/dev/null || true
run_root rmdir "${SHARE_DIR}" 2>/dev/null || true
run_root rmdir "${PREFIX}/share" 2>/dev/null || true
run_root rmdir "${BIN_DIR}" 2>/dev/null || true
run_root rmdir "${PREFIX}" 2>/dev/null || true

log "uninstall complete"
