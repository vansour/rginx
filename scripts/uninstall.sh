#!/usr/bin/env bash
set -euo pipefail

PREFIX="/usr"
CONFIG_DIR="/etc/rginx"
PURGE_CONFIG=0
YES=0
SUDO=""

usage() {
    cat <<'EOF'
Usage: uninstall.sh [options]

Options:
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

    for target in "${SBIN_DIR}" "${CONFIG_DIR}" "${DOC_DIR}" "${SHARE_DIR}" "${SYSTEMD_UNIT_DIR}"; do
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

systemd_is_available() {
    [[ -d /run/systemd/system ]] && have systemctl
}

disable_systemd_unit() {
    if ! systemd_is_available; then
        return
    fi

    if [[ ! -f "${SYSTEMD_UNIT_PATH}" ]]; then
        return
    fi

    log "stopping systemd unit: ${SYSTEMD_UNIT_NAME}"
    run_root systemctl disable --now "${SYSTEMD_UNIT_NAME}" >/dev/null 2>&1 || true
    run_root systemctl reset-failed "${SYSTEMD_UNIT_NAME}" >/dev/null 2>&1 || true
}

remove_systemd_unit() {
    if [[ -f "${SYSTEMD_UNIT_PATH}" ]]; then
        run_root rm -f "${SYSTEMD_UNIT_PATH}"
        log "removed systemd unit: ${SYSTEMD_UNIT_PATH}"
    fi

    if systemd_is_available; then
        run_root systemctl daemon-reload
    fi
}

SBIN_DIR="/usr/sbin"
SHARE_DIR="/usr/share/rginx"
DOC_DIR="/usr/share/doc/rginx"
STATE_DIR="/var/lib/rginx"
SYSTEMD_UNIT_DIR="/etc/systemd/system"
SYSTEMD_UNIT_NAME="rginx.service"
SYSTEMD_UNIT_PATH="${SYSTEMD_UNIT_DIR}/${SYSTEMD_UNIT_NAME}"

prepare_privileges

if [[ "${YES}" -ne 1 ]]; then
    printf 'This will remove:\n'
    printf '  - stop/disable %s\n' "${SYSTEMD_UNIT_NAME}"
    printf '  - %s\n' "${SYSTEMD_UNIT_PATH}"
    printf '  - %s\n' "${SBIN_DIR}/rginx"
    printf '  - %s\n' "${SBIN_DIR}/rginx-uninstall"
    printf '  - %s\n' "${DOC_DIR}"
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

disable_systemd_unit
remove_systemd_unit
run_root rm -f "${SBIN_DIR}/rginx"
run_root rm -f "${SBIN_DIR}/rginx-uninstall"
run_root rm -rf "${DOC_DIR}"

if [[ "${PURGE_CONFIG}" -eq 1 ]]; then
    run_root rm -rf "${CONFIG_DIR}"
    run_root rmdir "$(dirname "${CONFIG_DIR}")" 2>/dev/null || true
    log "removed config dir: ${CONFIG_DIR}"
else
    log "preserved config dir: ${CONFIG_DIR}"
fi

run_root rmdir "$(dirname "${DOC_DIR}")" 2>/dev/null || true
run_root rmdir "${SHARE_DIR}" 2>/dev/null || true
run_root rmdir "${STATE_DIR}" 2>/dev/null || true
run_root rmdir "${PREFIX}/share" 2>/dev/null || true
run_root rmdir "${SBIN_DIR}" 2>/dev/null || true

log "uninstall complete"
