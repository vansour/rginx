#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

PACKAGE_NAME="rginx"
MAINTAINER="vansour <packages@rginx.invalid>"
HOMEPAGE="https://github.com/vansour/rginx"
OUT_DIR="${ROOT_DIR}/target/debian"
SKIP_BUILD=0
BINARY_PATH=""

usage() {
    cat <<'EOF'
Usage: build-deb.sh [options]

Build a Debian package that can be installed with:
  sudo apt install ./target/debian/rginx_<version>_<arch>.deb

Options:
  --out-dir <dir>
      Output directory for the generated .deb, default: target/debian
  --maintainer <name or email>
      Maintainer value written into DEBIAN/control
  --skip-build
      Reuse existing target/release/rginx instead of running cargo build --release
  --binary <path>
      Use an explicit compiled binary path instead of target/release/rginx
  -h, --help
      Show help
EOF
}

log() {
    printf '[build-deb] %s\n' "$*"
}

die() {
    printf '[build-deb] error: %s\n' "$*" >&2
    exit 1
}

have() {
    command -v "$1" >/dev/null 2>&1
}

workspace_version() {
    awk '
        /^\[workspace\.package\]/ { in_section=1; next }
        /^\[/ { if (in_section) exit }
        in_section && $1 == "version" {
            gsub(/"/, "", $3)
            print $3
            exit
        }
    ' "${ROOT_DIR}/Cargo.toml"
}

debian_version() {
    local version="$1"
    version="$(printf '%s' "${version}" | sed 's/-/~/1')"
    printf '%s-1\n' "${version}"
}

deb_arch() {
    case "$(uname -m)" in
        x86_64|amd64)
            printf 'amd64\n'
            ;;
        aarch64|arm64)
            printf 'arm64\n'
            ;;
        *)
            die "unsupported architecture: $(uname -m)"
            ;;
    esac
}

install_doc_if_exists() {
    local source_path="$1"
    local target_dir="$2"

    if [[ -f "${source_path}" ]]; then
        install -m 644 "${source_path}" "${target_dir}/$(basename "${source_path}")"
    fi
}

render_control() {
    local template_path="$1"
    local output_path="$2"
    local package="$3"
    local version="$4"
    local arch="$5"
    local maintainer="$6"
    local installed_size="$7"
    local homepage="$8"

    sed \
        -e "s|@PACKAGE@|${package}|g" \
        -e "s|@VERSION@|${version}|g" \
        -e "s|@ARCH@|${arch}|g" \
        -e "s|@MAINTAINER@|${maintainer}|g" \
        -e "s|@INSTALLED_SIZE@|${installed_size}|g" \
        -e "s|@HOMEPAGE@|${homepage}|g" \
        "${template_path}" > "${output_path}"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --out-dir)
            [[ $# -ge 2 ]] || die "--out-dir requires a value"
            OUT_DIR="$2"
            shift 2
            ;;
        --maintainer)
            [[ $# -ge 2 ]] || die "--maintainer requires a value"
            MAINTAINER="$2"
            shift 2
            ;;
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --binary)
            [[ $# -ge 2 ]] || die "--binary requires a value"
            BINARY_PATH="$2"
            shift 2
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

have cargo || die "cargo is required"
have dpkg-deb || die "dpkg-deb is required"

VERSION="$(workspace_version)"
[[ -n "${VERSION}" ]] || die "failed to resolve workspace version from Cargo.toml"
DEB_VERSION="$(debian_version "${VERSION}")"
ARCH="$(deb_arch)"
PACKAGE_FILENAME="${PACKAGE_NAME}_${DEB_VERSION}_${ARCH}.deb"
PACKAGE_PATH="${OUT_DIR}/${PACKAGE_FILENAME}"
STAGE_DIR="${OUT_DIR}/build/${PACKAGE_NAME}_${DEB_VERSION}_${ARCH}"
DEBIAN_DIR="${STAGE_DIR}/DEBIAN"

if [[ "${SKIP_BUILD}" -eq 0 ]]; then
    log "building release binary"
    cargo build --locked --release -p rginx --manifest-path "${ROOT_DIR}/Cargo.toml"
fi

BINARY_PATH="${BINARY_PATH:-${ROOT_DIR}/target/release/rginx}"
[[ -x "${BINARY_PATH}" ]] || die "release binary not found: ${BINARY_PATH}"

log "staging package root"
rm -rf "${STAGE_DIR}"
install -d \
    "${DEBIAN_DIR}" \
    "${STAGE_DIR}/usr/sbin" \
    "${STAGE_DIR}/lib/systemd/system" \
    "${STAGE_DIR}/usr/share/doc/${PACKAGE_NAME}" \
    "${STAGE_DIR}/etc/rginx/conf.d"

install -m 755 "${BINARY_PATH}" "${STAGE_DIR}/usr/sbin/rginx"
install -m 644 "${ROOT_DIR}/configs/rginx.ron" "${STAGE_DIR}/etc/rginx/rginx.ron"

if compgen -G "${ROOT_DIR}/configs/conf.d/*.ron" >/dev/null; then
    for fragment in "${ROOT_DIR}"/configs/conf.d/*.ron; do
        install -m 644 "${fragment}" "${STAGE_DIR}/etc/rginx/conf.d/$(basename "${fragment}")"
    done
fi

install -m 644 "${ROOT_DIR}/deploy/systemd/rginx.service" \
    "${STAGE_DIR}/lib/systemd/system/rginx.service"

install_doc_if_exists "${ROOT_DIR}/README.md" "${STAGE_DIR}/usr/share/doc/${PACKAGE_NAME}"
install_doc_if_exists "${ROOT_DIR}/LICENSE" "${STAGE_DIR}/usr/share/doc/${PACKAGE_NAME}"
install_doc_if_exists "${ROOT_DIR}/LICENSE-APACHE" "${STAGE_DIR}/usr/share/doc/${PACKAGE_NAME}"
install_doc_if_exists "${ROOT_DIR}/LICENSE-MIT" "${STAGE_DIR}/usr/share/doc/${PACKAGE_NAME}"

INSTALLED_SIZE="$(
    find "${STAGE_DIR}" -mindepth 1 -maxdepth 1 ! -name DEBIAN -exec du -sk {} + |
        awk '{sum += $1} END {print sum + 0}'
)"

render_control \
    "${ROOT_DIR}/packaging/apt/control.in" \
    "${DEBIAN_DIR}/control" \
    "${PACKAGE_NAME}" \
    "${DEB_VERSION}" \
    "${ARCH}" \
    "${MAINTAINER}" \
    "${INSTALLED_SIZE}" \
    "${HOMEPAGE}"

install -m 755 "${ROOT_DIR}/packaging/apt/postinst" "${DEBIAN_DIR}/postinst"
install -m 755 "${ROOT_DIR}/packaging/apt/prerm" "${DEBIAN_DIR}/prerm"
install -m 755 "${ROOT_DIR}/packaging/apt/postrm" "${DEBIAN_DIR}/postrm"

{
    printf '/etc/rginx/rginx.ron\n'
    if compgen -G "${STAGE_DIR}/etc/rginx/conf.d/*.ron" >/dev/null; then
        for fragment in "${STAGE_DIR}"/etc/rginx/conf.d/*.ron; do
            printf '/etc/rginx/conf.d/%s\n' "$(basename "${fragment}")"
        done
    fi
} > "${DEBIAN_DIR}/conffiles"

install -d "${OUT_DIR}"
log "building ${PACKAGE_FILENAME}"
dpkg-deb --root-owner-group --build "${STAGE_DIR}" "${PACKAGE_PATH}" >/dev/null

log "package ready: ${PACKAGE_PATH}"
