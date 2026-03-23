#!/usr/bin/env bash
set -euo pipefail

REPO_SLUG="vansour/rginx"

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_PATH=""
SCRIPT_DIR=""
LOCAL_ROOT=""

MODE="auto"
VERSION="latest"
PREFIX=""
CONFIG_DIR=""
FORCE=0
TMP_ROOT=""
SUDO=""

usage() {
    cat <<'EOF'
Usage: install.sh [options]

Options:
  --mode auto|source|archive|release
      auto    : 优先使用当前目录下的 release archive，其次源码构建，最后下载 GitHub Release
      source  : 从当前源码仓库执行 cargo build --release 后安装
      archive : 从当前解压后的 release archive 安装
      release : 直接从 GitHub Release 下载并安装
  --version <tag|latest>
      release 模式下使用的版本，默认 latest；latest 只解析最新稳定版
  --prefix <path>
      安装前缀。Linux 默认 /usr/local；macOS arm64 且存在 /opt/homebrew 时默认 /opt/homebrew
  --config-dir <path>
      活跃配置目录，默认 <prefix>/etc/rginx
  --force
      强制覆盖已存在的活跃配置文件
  -h, --help
      显示帮助
EOF
}

log() {
    printf '[install] %s\n' "$*"
}

die() {
    printf '[install] error: %s\n' "$*" >&2
    exit 1
}

have() {
    command -v "$1" >/dev/null 2>&1
}

cleanup() {
    if [[ -n "${TMP_ROOT}" && -d "${TMP_ROOT}" ]]; then
        rm -rf "${TMP_ROOT}"
    fi
}

trap cleanup EXIT

if [[ "${SCRIPT_SOURCE}" != "bash" && -f "${SCRIPT_SOURCE}" ]]; then
    SCRIPT_PATH="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)/$(basename "${SCRIPT_SOURCE}")"
    SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_PATH}")" && pwd)"

    if [[ "$(basename "${SCRIPT_DIR}")" == "scripts" ]]; then
        LOCAL_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
    else
        LOCAL_ROOT="${SCRIPT_DIR}"
    fi
fi

while [[ $# -gt 0 ]]; do
    case "$1" in
        --mode)
            [[ $# -ge 2 ]] || die "--mode requires a value"
            MODE="$2"
            shift 2
            ;;
        --version)
            [[ $# -ge 2 ]] || die "--version requires a value"
            VERSION="$2"
            shift 2
            ;;
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
        --force)
            FORCE=1
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

default_prefix() {
    local os arch
    os="$(uname -s)"
    arch="$(uname -m)"

    if [[ "${os}" == "Darwin" && "${arch}" == "arm64" && -d /opt/homebrew ]]; then
        printf '%s\n' "/opt/homebrew"
        return
    fi

    printf '%s\n' "/usr/local"
}

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
            have sudo || die "install target requires elevated privileges, but sudo is not available"
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

detect_release_platform() {
    local os arch
    os="$(uname -s)"
    arch="$(uname -m)"

    case "${os}" in
        Linux)
            RELEASE_OS="linux"
            ;;
        Darwin)
            RELEASE_OS="darwin"
            ;;
        *)
            die "unsupported operating system: ${os}"
            ;;
    esac

    case "${arch}" in
        x86_64|amd64)
            RELEASE_ARCH="amd64"
            ;;
        aarch64|arm64)
            RELEASE_ARCH="arm64"
            ;;
        *)
            die "unsupported architecture: ${arch}"
            ;;
    esac
}

is_source_root() {
    local path="${1:-}"

    [[ -n "${path}" ]] || return 1
    [[ -f "${path}/Cargo.toml" ]] || return 1
    [[ -d "${path}/crates/rginx-app" ]] || return 1
    [[ -d "${path}/configs" ]] || return 1
}

is_archive_root() {
    local path="${1:-}"

    [[ -n "${path}" ]] || return 1
    [[ -x "${path}/rginx" ]] || return 1
    [[ -d "${path}/configs" ]] || return 1
    [[ -f "${path}/scripts/uninstall.sh" ]] || return 1
}

resolve_latest_release() {
    local response tag

    have curl || die "curl is required to resolve the latest release"

    response="$(curl -fsSL "https://api.github.com/repos/${REPO_SLUG}/releases/latest")" \
        || die "failed to resolve the latest stable release; use --version or --mode source"

    tag="$(printf '%s' "${response}" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
    [[ -n "${tag}" ]] || die "unable to parse the latest release tag from GitHub API"

    printf '%s\n' "${tag}"
}

resolve_mode() {
    case "${MODE}" in
        auto)
            if is_archive_root "${LOCAL_ROOT}"; then
                MODE="archive"
            elif is_source_root "${LOCAL_ROOT}" && have cargo; then
                MODE="source"
            else
                MODE="release"
            fi
            ;;
        source|archive|release)
            ;;
        *)
            die "unsupported mode: ${MODE}"
            ;;
    esac
}

stage_from_source() {
    have cargo || die "cargo is required for --mode source"
    is_source_root "${LOCAL_ROOT}" || die "--mode source must be run from the repository scripts directory"

    log "building rginx from source"
    cargo build --locked --release -p rginx --manifest-path "${LOCAL_ROOT}/Cargo.toml"

    STAGED_ROOT="${LOCAL_ROOT}"
    STAGED_BIN="${LOCAL_ROOT}/target/release/rginx"
    STAGED_UNINSTALL="${LOCAL_ROOT}/scripts/uninstall.sh"
}

stage_from_archive() {
    is_archive_root "${LOCAL_ROOT}" || die "--mode archive must be run from an extracted release archive"

    STAGED_ROOT="${LOCAL_ROOT}"
    STAGED_BIN="${LOCAL_ROOT}/rginx"
    STAGED_UNINSTALL="${LOCAL_ROOT}/scripts/uninstall.sh"
}

stage_from_release() {
    local tag archive_name archive_url unpack_dir

    have curl || die "curl is required for --mode release"
    have tar || die "tar is required for --mode release"

    detect_release_platform

    tag="${VERSION}"
    if [[ "${tag}" == "latest" ]]; then
        tag="$(resolve_latest_release)"
    fi

    TMP_ROOT="$(mktemp -d)"
    archive_name="rginx-${tag}-${RELEASE_OS}-${RELEASE_ARCH}.tar.gz"
    archive_url="https://github.com/${REPO_SLUG}/releases/download/${tag}/${archive_name}"

    log "downloading ${archive_url}"
    curl -fL --retry 3 --retry-delay 1 --connect-timeout 10 "${archive_url}" -o "${TMP_ROOT}/${archive_name}" \
        || die "failed to download ${archive_url}"

    tar -C "${TMP_ROOT}" -xzf "${TMP_ROOT}/${archive_name}"
    unpack_dir="${TMP_ROOT}/rginx-${tag#v}-${RELEASE_OS}-${RELEASE_ARCH}"
    [[ -d "${unpack_dir}" ]] || die "unexpected archive layout: ${archive_name}"

    STAGED_ROOT="${unpack_dir}"
    STAGED_BIN="${unpack_dir}/rginx"
    STAGED_UNINSTALL="${unpack_dir}/scripts/uninstall.sh"
}

PREFIX="${PREFIX:-$(default_prefix)}"
CONFIG_DIR="${CONFIG_DIR:-${PREFIX}/etc/rginx}"
BIN_DIR="${PREFIX}/bin"
SHARE_DIR="${PREFIX}/share/rginx"
DOC_DIR="${PREFIX}/share/doc/rginx"
EXAMPLES_DIR="${SHARE_DIR}/configs"
MANIFEST_PATH="${SHARE_DIR}/install-manifest.txt"
ACTIVE_CONFIG="${CONFIG_DIR}/rginx.ron"

resolve_mode

case "${MODE}" in
    source)
        stage_from_source
        ;;
    archive)
        stage_from_archive
        ;;
    release)
        stage_from_release
        ;;
esac

[[ -x "${STAGED_BIN}" ]] || die "staged rginx binary not found: ${STAGED_BIN}"
[[ -d "${STAGED_ROOT}/configs" ]] || die "staged configs directory not found: ${STAGED_ROOT}/configs"
[[ -f "${STAGED_UNINSTALL}" ]] || die "staged uninstall script not found: ${STAGED_UNINSTALL}"

prepare_privileges

log "resolved install mode: ${MODE}"
log "installing to prefix ${PREFIX}"
run_root install -d "${BIN_DIR}" "${CONFIG_DIR}" "${SHARE_DIR}" "${DOC_DIR}"
run_root rm -rf "${EXAMPLES_DIR}"
run_root install -d "${EXAMPLES_DIR}"

run_root install -m 755 "${STAGED_BIN}" "${BIN_DIR}/rginx"
run_root install -m 755 "${STAGED_UNINSTALL}" "${BIN_DIR}/rginx-uninstall"

for doc in README.md CHANGELOG.md LICENSE LICENSE-APACHE LICENSE-MIT; do
    if [[ -f "${STAGED_ROOT}/${doc}" ]]; then
        run_root install -m 644 "${STAGED_ROOT}/${doc}" "${DOC_DIR}/${doc}"
    elif [[ -n "${LOCAL_ROOT}" && -f "${LOCAL_ROOT}/${doc}" ]]; then
        run_root install -m 644 "${LOCAL_ROOT}/${doc}" "${DOC_DIR}/${doc}"
    fi
done

run_root cp -R "${STAGED_ROOT}/configs/." "${EXAMPLES_DIR}/"

if [[ ! -f "${ACTIVE_CONFIG}" || "${FORCE}" -eq 1 ]]; then
    run_root install -m 644 "${STAGED_ROOT}/configs/rginx.ron" "${ACTIVE_CONFIG}"
    ACTIVE_CONFIG_RESULT="installed"
else
    ACTIVE_CONFIG_RESULT="preserved"
fi

TMP_MANIFEST="$(mktemp)"
{
    printf 'prefix=%q\n' "${PREFIX}"
    printf 'config_dir=%q\n' "${CONFIG_DIR}"
    printf 'bin_dir=%q\n' "${BIN_DIR}"
    printf 'share_dir=%q\n' "${SHARE_DIR}"
    printf 'doc_dir=%q\n' "${DOC_DIR}"
    printf 'examples_dir=%q\n' "${EXAMPLES_DIR}"
    printf 'active_config=%q\n' "${ACTIVE_CONFIG}"
} > "${TMP_MANIFEST}"
run_root install -m 644 "${TMP_MANIFEST}" "${MANIFEST_PATH}"
rm -f "${TMP_MANIFEST}"

log "binary: ${BIN_DIR}/rginx"
log "uninstall: ${BIN_DIR}/rginx-uninstall"
log "active config (${ACTIVE_CONFIG_RESULT}): ${ACTIVE_CONFIG}"
log "example configs: ${EXAMPLES_DIR}"
log "default config search will now pick ${ACTIVE_CONFIG} when running ${BIN_DIR}/rginx"

if [[ "${CONFIG_DIR}" != "${PREFIX}/etc/rginx" ]]; then
    log "custom config dir detected; run with --config ${ACTIVE_CONFIG} or set RGINX_CONFIG=${ACTIVE_CONFIG}"
fi
