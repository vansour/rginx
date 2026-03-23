#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

SOURCE_DIR="${ROOT_DIR}/wiki"
REMOTE_URL=""
COMMIT_MESSAGE=""
PUSH=1
TMP_ROOT=""

usage() {
    cat <<'EOF'
Usage: sync-wiki.sh [options]

Options:
  --remote <url>
      显式指定 GitHub Wiki 仓库地址；默认从 origin 推导
  --message <text>
      提交到 Wiki 仓库时使用的 commit message
  --no-push
      只生成本地同步结果和 commit，不执行 git push
  -h, --help
      显示帮助
EOF
}

log() {
    printf '[wiki-sync] %s\n' "$*"
}

die() {
    printf '[wiki-sync] error: %s\n' "$*" >&2
    exit 1
}

cleanup() {
    if [[ -n "${TMP_ROOT}" && -d "${TMP_ROOT}" ]]; then
        rm -rf "${TMP_ROOT}"
    fi
}

trap cleanup EXIT

while [[ $# -gt 0 ]]; do
    case "$1" in
        --remote)
            [[ $# -ge 2 ]] || die "--remote requires a value"
            REMOTE_URL="$2"
            shift 2
            ;;
        --message)
            [[ $# -ge 2 ]] || die "--message requires a value"
            COMMIT_MESSAGE="$2"
            shift 2
            ;;
        --no-push)
            PUSH=0
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

[[ -d "${SOURCE_DIR}" ]] || die "wiki source directory not found: ${SOURCE_DIR}"
[[ -f "${SOURCE_DIR}/Home.md" ]] || die "wiki source directory must contain Home.md"

derive_remote_url() {
    local origin_url
    origin_url="$(git -C "${ROOT_DIR}" remote get-url origin)" || die "failed to read origin remote"

    case "${origin_url}" in
        git@github.com:*.git)
            printf '%s.wiki.git\n' "${origin_url%.git}"
            ;;
        https://github.com/*.git)
            printf '%s.wiki.git\n' "${origin_url%.git}"
            ;;
        https://github.com/*)
            printf '%s.wiki.git\n' "${origin_url}"
            ;;
        ssh://git@github.com/*.git)
            printf '%s.wiki.git\n' "${origin_url%.git}"
            ;;
        *)
            die "unsupported origin remote format: ${origin_url}"
            ;;
    esac
}

copy_source_tree() {
    local source_path target_path entry

    find "${CHECKOUT_DIR}" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +

    for source_path in "${SOURCE_DIR}"/*; do
        [[ -e "${source_path}" ]] || continue

        entry="$(basename "${source_path}")"
        if [[ "${entry}" == "README.md" ]]; then
            continue
        fi

        target_path="${CHECKOUT_DIR}/${entry}"
        if [[ -d "${source_path}" ]]; then
            cp -R "${source_path}" "${target_path}"
        else
            cp "${source_path}" "${target_path}"
        fi
    done
}

REMOTE_URL="${REMOTE_URL:-$(derive_remote_url)}"
TMP_ROOT="$(mktemp -d)"
CHECKOUT_DIR="${TMP_ROOT}/wiki-repo"

log "cloning ${REMOTE_URL}"
git clone --quiet "${REMOTE_URL}" "${CHECKOUT_DIR}"

WIKI_BRANCH="$(git -C "${CHECKOUT_DIR}" branch --show-current)"
[[ -n "${WIKI_BRANCH}" ]] || die "failed to resolve checked out wiki branch"

copy_source_tree

git -C "${CHECKOUT_DIR}" add --all

if [[ -z "$(git -C "${CHECKOUT_DIR}" status --short)" ]]; then
    log "wiki already up to date"
    exit 0
fi

if [[ -z "${COMMIT_MESSAGE}" ]]; then
    SOURCE_BRANCH="$(git -C "${ROOT_DIR}" branch --show-current)"
    SOURCE_SHA="$(git -C "${ROOT_DIR}" rev-parse --short HEAD)"
    COMMIT_MESSAGE="docs: sync wiki from ${SOURCE_BRANCH}@${SOURCE_SHA}"
fi

log "creating wiki commit"
git -C "${CHECKOUT_DIR}" commit -m "${COMMIT_MESSAGE}" >/dev/null

if [[ "${PUSH}" -eq 1 ]]; then
    log "pushing to ${WIKI_BRANCH}"
    git -C "${CHECKOUT_DIR}" push origin "HEAD:${WIKI_BRANCH}"
else
    log "skipping push (--no-push)"
fi

NEW_SHA="$(git -C "${CHECKOUT_DIR}" rev-parse --short HEAD)"
log "wiki sync complete: ${NEW_SHA}"
