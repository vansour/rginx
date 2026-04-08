#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

REPO_ROOT=""
SUITE="stable"
COMPONENT="main"
ORIGIN="rginx"
LABEL="rginx"
GPG_KEY=""
EXPORT_KEY_PATH=""
DEBS=()
DEB_DIR=""

usage() {
    cat <<'EOF'
Usage: publish-apt-repo.sh --repo-root <dir> --gpg-key <key-id> [options]

Publish one or more .deb packages into a static APT repository layout.

Examples:
  ./scripts/build-deb.sh
  ./scripts/publish-apt-repo.sh \
      --repo-root ./target/apt-repo \
      --deb ./target/debian/rginx_0.1.3~rc.2-1_amd64.deb \
      --gpg-key packages@example.com \
      --export-key ./target/apt-repo/rginx-archive-keyring.gpg

Options:
  --repo-root <dir>
      Repository output directory
  --deb <path>
      Package to publish, repeatable
  --deb-dir <dir>
      Publish every .deb file from a directory
  --suite <name>
      APT suite/codename, default: stable
  --component <name>
      APT component, default: main
  --origin <text>
      Release file Origin field, default: rginx
  --label <text>
      Release file Label field, default: rginx
  --gpg-key <key-id>
      GPG key id, fingerprint, or email used to sign Release/InRelease
  --export-key <path>
      Export the public key to this path as a binary keyring
  -h, --help
      Show help
EOF
}

log() {
    printf '[apt-publish] %s\n' "$*"
}

die() {
    printf '[apt-publish] error: %s\n' "$*" >&2
    exit 1
}

have() {
    command -v "$1" >/dev/null 2>&1
}

append_debs_from_dir() {
    local dir="$1"
    local found=0

    [[ -d "${dir}" ]] || die "deb directory not found: ${dir}"
    for deb in "${dir}"/*.deb; do
        [[ -e "${deb}" ]] || continue
        DEBS+=("${deb}")
        found=1
    done

    [[ "${found}" -eq 1 ]] || die "no .deb files found in ${dir}"
}

copy_packages_into_pool() {
    local pool_dir="$1"
    shift

    install -d "${pool_dir}"
    for deb in "$@"; do
        [[ -f "${deb}" ]] || die "package not found: ${deb}"
        install -m 644 "${deb}" "${pool_dir}/$(basename "${deb}")"
    done
}

find_repo_architectures() {
    local pool_dir="$1"
    find "${pool_dir}" -type f -name '*.deb' -print0 | while IFS= read -r -d '' deb; do
        dpkg-deb -f "${deb}" Architecture
    done | sort -u
}

write_packages_indexes() {
    local repo_root="$1"
    local pool_dir="$2"
    local suite_dir="$3"
    local component="$4"
    shift 4
    local arch
    local pool_rel="${pool_dir#${repo_root}/}"

    for arch in "$@"; do
        local binary_dir="${suite_dir}/${component}/binary-${arch}"
        install -d "${binary_dir}"
        (
            cd "${repo_root}"
            dpkg-scanpackages -m -a "${arch}" "${pool_rel}" /dev/null
        ) > "${binary_dir}/Packages"
        gzip -n -9c "${binary_dir}/Packages" > "${binary_dir}/Packages.gz"
    done
}

release_file_entries() {
    local suite_dir="$1"
    local checksum_cmd="$2"

    find "${suite_dir}" -type f \
        ! -name 'Release' \
        ! -name 'InRelease' \
        ! -name 'Release.gpg' \
        -print0 \
        | sort -z \
        | while IFS= read -r -d '' file; do
            local rel_path size checksum
            rel_path="${file#${suite_dir}/}"
            size="$(stat -c '%s' "${file}")"
            checksum="$(${checksum_cmd} "${file}" | awk '{print $1}')"
            printf ' %s %16s %s\n' "${checksum}" "${size}" "${rel_path}"
        done
}

write_release_file() {
    local suite_dir="$1"
    local origin="$2"
    local label="$3"
    local suite="$4"
    local component="$5"
    shift 5
    local architectures=("$@")
    local release_path="${suite_dir}/Release"

    {
        printf 'Origin: %s\n' "${origin}"
        printf 'Label: %s\n' "${label}"
        printf 'Suite: %s\n' "${suite}"
        printf 'Codename: %s\n' "${suite}"
        printf 'Date: %s\n' "$(LC_ALL=C date -Ru)"
        printf 'Architectures: %s\n' "${architectures[*]}"
        printf 'Components: %s\n' "${component}"
        printf 'MD5Sum:\n'
        release_file_entries "${suite_dir}" md5sum
        printf 'SHA256:\n'
        release_file_entries "${suite_dir}" sha256sum
    } > "${release_path}"
}

sign_release() {
    local suite_dir="$1"
    local gpg_key="$2"
    local -a gpg_args=(--batch --yes --local-user "${gpg_key}")

    if [[ -n "${APT_GPG_PASSPHRASE:-}" ]]; then
        gpg_args+=(--pinentry-mode loopback --passphrase "${APT_GPG_PASSPHRASE}")
    fi

    gpg "${gpg_args[@]}" \
        --output "${suite_dir}/InRelease" \
        --clearsign "${suite_dir}/Release"

    gpg "${gpg_args[@]}" \
        --output "${suite_dir}/Release.gpg" \
        --detach-sign "${suite_dir}/Release"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --repo-root)
            [[ $# -ge 2 ]] || die "--repo-root requires a value"
            REPO_ROOT="$2"
            shift 2
            ;;
        --deb)
            [[ $# -ge 2 ]] || die "--deb requires a value"
            DEBS+=("$2")
            shift 2
            ;;
        --deb-dir)
            [[ $# -ge 2 ]] || die "--deb-dir requires a value"
            DEB_DIR="$2"
            shift 2
            ;;
        --suite)
            [[ $# -ge 2 ]] || die "--suite requires a value"
            SUITE="$2"
            shift 2
            ;;
        --component)
            [[ $# -ge 2 ]] || die "--component requires a value"
            COMPONENT="$2"
            shift 2
            ;;
        --origin)
            [[ $# -ge 2 ]] || die "--origin requires a value"
            ORIGIN="$2"
            shift 2
            ;;
        --label)
            [[ $# -ge 2 ]] || die "--label requires a value"
            LABEL="$2"
            shift 2
            ;;
        --gpg-key)
            [[ $# -ge 2 ]] || die "--gpg-key requires a value"
            GPG_KEY="$2"
            shift 2
            ;;
        --export-key)
            [[ $# -ge 2 ]] || die "--export-key requires a value"
            EXPORT_KEY_PATH="$2"
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

[[ -n "${REPO_ROOT}" ]] || die "--repo-root is required"
[[ -n "${GPG_KEY}" ]] || die "--gpg-key is required"

have dpkg-deb || die "dpkg-deb is required"
have dpkg-scanpackages || die "dpkg-scanpackages is required"
have gpg || die "gpg is required"

if [[ -n "${DEB_DIR}" ]]; then
    append_debs_from_dir "${DEB_DIR}"
fi

[[ "${#DEBS[@]}" -gt 0 ]] || die "at least one --deb or --deb-dir is required"

POOL_DIR="${REPO_ROOT}/pool/${COMPONENT}/r/${PACKAGE_NAME:-rginx}"
SUITE_DIR="${REPO_ROOT}/dists/${SUITE}"

log "copying packages into pool"
copy_packages_into_pool "${POOL_DIR}" "${DEBS[@]}"

mapfile -t ARCHITECTURES < <(find_repo_architectures "${POOL_DIR}")
[[ "${#ARCHITECTURES[@]}" -gt 0 ]] || die "no architectures found in ${POOL_DIR}"

log "writing Packages indexes for: ${ARCHITECTURES[*]}"
write_packages_indexes "${REPO_ROOT}" "${POOL_DIR}" "${SUITE_DIR}" "${COMPONENT}" "${ARCHITECTURES[@]}"

log "writing Release metadata"
write_release_file "${SUITE_DIR}" "${ORIGIN}" "${LABEL}" "${SUITE}" "${COMPONENT}" "${ARCHITECTURES[@]}"

log "signing Release metadata"
sign_release "${SUITE_DIR}" "${GPG_KEY}"

if [[ -n "${EXPORT_KEY_PATH}" ]]; then
    install -d "$(dirname "${EXPORT_KEY_PATH}")"
    gpg --batch --yes --output "${EXPORT_KEY_PATH}" --export "${GPG_KEY}"
    log "exported public key: ${EXPORT_KEY_PATH}"
fi

log "APT repository ready under ${REPO_ROOT}"
