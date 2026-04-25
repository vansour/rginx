#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
source "${SCRIPT_DIR}/fuzz-common.sh"

TAG=""
ALLOW_DIRTY=0
SKIP_FETCH=0

usage() {
    cat <<'EOF'
Usage: prepare-release.sh --tag <tag> [options]

Options:
  --tag <tag>
      目标 release tag，例如 v0.1.2 或 v0.1.2-rc.8
  --allow-dirty
      允许在有未提交改动的工作区内执行
  --skip-fetch
      跳过 git fetch origin main 与远端 tag 冲突检查
  -h, --help
      显示帮助
EOF
}

log() {
    printf '[release-prep] %s\n' "$*"
}

die() {
    printf '[release-prep] error: %s\n' "$*" >&2
    exit 1
}

have() {
    command -v "$1" >/dev/null 2>&1
}

run_step() {
    log "running: $*"
    "$@"
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

while [[ $# -gt 0 ]]; do
    case "$1" in
        --tag)
            [[ $# -ge 2 ]] || die "--tag requires a value"
            TAG="$2"
            shift 2
            ;;
        --allow-dirty)
            ALLOW_DIRTY=1
            shift
            ;;
        --skip-fetch)
            SKIP_FETCH=1
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

[[ -n "${TAG}" ]] || die "--tag is required"
[[ "${TAG}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.]+)?$ ]] || die "release tag must match v<major>.<minor>.<patch> or v<major>.<minor>.<patch>-<prerelease>"

have git || die "git is required"
have cargo || die "cargo is required"

cd "${ROOT_DIR}"

VERSION="${TAG#v}"
PRERELEASE=0
if [[ "${TAG}" == *-* ]]; then
    PRERELEASE=1
fi

if [[ "${PRERELEASE}" -eq 1 ]]; then
    have rustup || die "rustup is required for prerelease fuzz smoke"
    FUZZ_TOOLCHAIN="$(fuzz_toolchain_channel "${ROOT_DIR}/fuzz")"
    [[ -n "${FUZZ_TOOLCHAIN}" ]] || die "failed to resolve fuzz toolchain from fuzz/rust-toolchain.toml"
    rustup toolchain list | grep -Fq "${FUZZ_TOOLCHAIN}" \
        || die "fuzz toolchain ${FUZZ_TOOLCHAIN} is required for prerelease fuzz smoke (run: rustup toolchain install ${FUZZ_TOOLCHAIN})"
    cargo fuzz --help >/dev/null 2>&1 \
        || die "cargo-fuzz is required for prerelease fuzz smoke (run: cargo install cargo-fuzz)"
fi

CURRENT_BRANCH="$(git branch --show-current)"
if [[ "${PRERELEASE}" -ne 1 ]] && [[ "${CURRENT_BRANCH}" != "main" ]]; then
    die "stable release prep must run from the main branch"
fi

if [[ "${ALLOW_DIRTY}" -ne 1 ]] && [[ -n "$(git status --short)" ]]; then
    die "worktree is not clean; commit or stash changes first"
fi

LOCAL_HEAD="$(git rev-parse HEAD)"

if [[ "${SKIP_FETCH}" -ne 1 ]]; then
    RELEASE_BRANCH="release/${TAG}"
    run_step git fetch --no-tags origin main

    REMOTE_HEAD="$(git rev-parse origin/main)"
    RELEASE_BRANCH_EXISTS=0
    RELEASE_BRANCH_HEAD=""

    if git ls-remote --exit-code --heads origin "${RELEASE_BRANCH}" >/dev/null 2>&1; then
        run_step git fetch --no-tags origin "+refs/heads/${RELEASE_BRANCH}:refs/remotes/origin/${RELEASE_BRANCH}"
        RELEASE_BRANCH_EXISTS=1
        RELEASE_BRANCH_HEAD="$(git rev-parse "origin/${RELEASE_BRANCH}")"
    fi

    if [[ "${PRERELEASE}" -eq 1 ]]; then
        if git merge-base --is-ancestor "${LOCAL_HEAD}" origin/main; then
            :
        elif [[ "${RELEASE_BRANCH_EXISTS}" -eq 1 ]] && git merge-base --is-ancestor "${LOCAL_HEAD}" "origin/${RELEASE_BRANCH}"; then
            :
        elif [[ "${RELEASE_BRANCH_EXISTS}" -eq 1 ]]; then
            die "prerelease tag ${TAG} must point to a commit reachable from origin/main (${REMOTE_HEAD}) or origin/${RELEASE_BRANCH} (${RELEASE_BRANCH_HEAD}), got ${LOCAL_HEAD}"
        else
            die "prerelease tag ${TAG} must point to a commit reachable from origin/main (${REMOTE_HEAD}); release branch origin/${RELEASE_BRANCH} was not found, got ${LOCAL_HEAD}"
        fi
    else
        [[ "${LOCAL_HEAD}" == "${REMOTE_HEAD}" ]] || die "HEAD (${LOCAL_HEAD}) does not match origin/main (${REMOTE_HEAD})"
    fi
else
    log "skip-fetch enabled; skipping origin/main ancestry and remote tag checks"
fi

if git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null 2>&1; then
    die "local tag ${TAG} already exists"
fi

if [[ "${SKIP_FETCH}" -ne 1 ]] && git ls-remote --exit-code --tags origin "refs/tags/${TAG}" >/dev/null 2>&1; then
    die "remote tag ${TAG} already exists on origin"
fi

WORKSPACE_VERSION="$(workspace_version)"
[[ -n "${WORKSPACE_VERSION}" ]] || die "failed to resolve workspace version from Cargo.toml"
[[ "${WORKSPACE_VERSION}" == "${VERSION}" ]] || die "workspace version ${WORKSPACE_VERSION} does not match tag ${TAG}"

run_step cargo fmt --all --check
run_step ./scripts/test-fast.sh
run_step ./scripts/run-clippy-gate.sh
run_step ./scripts/test-slow.sh
run_step ./scripts/run-tls-gate.sh
run_step ./scripts/run-http3-release-gate.sh --soak-iterations 1
run_step ./scripts/test-control-plane-compose.sh
if [[ "${PRERELEASE}" -eq 1 ]]; then
    run_step ./scripts/run-fuzz-smoke.sh --seconds 10
fi

log "running: cargo run -p rginx -- --version"
VERSION_OUTPUT="$(cargo run -p rginx -- --version | tail -n 1)"
[[ "${VERSION_OUTPUT}" == "rginx ${VERSION}" ]] || die "binary version mismatch: expected 'rginx ${VERSION}', got '${VERSION_OUTPUT}'"

cat <<EOF

[release-prep] preflight checks passed for ${TAG}
[release-prep] next steps:
  1. Create the tag: git tag ${TAG}
  2. Push the tag:   git push origin ${TAG}
  3. Verify the GitHub Release artifacts and release notes

EOF
