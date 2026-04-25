# rginx v0.1.3-rc.13

Updated: `2026-04-25`

## Summary

`v0.1.3-rc.13` is now a single-binary release candidate for `rginx`.

This tag keeps the HTTP/3 alignment work and prerelease fuzz gates intact while
removing the stale control-plane product line from the workspace, release
workflow, and repository packaging. The published artifacts for this tag are
now only the Linux `rginx` binaries and their checksums.

## Highlights

### Deterministic Fuzzing Baseline

- the fuzz harness now covers five high-risk input surfaces:
  - `proxy_protocol`
  - `config_preprocess`
  - `ocsp_response`
  - `certificate_inspect`
  - `ocsp_responder_discovery`
- versioned `*.seed` corpora are now the default replay source for smoke and
  coverage
- local auto-discovered corpus files remain ignored by git and no longer pollute
  default smoke / coverage runs
- target-specific dictionaries and `fuzz/options/<target>.options` now provide a
  reproducible libFuzzer baseline for CI and prerelease verification

### Prerelease Gate Tightening

- `scripts/prepare-release.sh` now runs `./scripts/run-fuzz-smoke.sh --seconds 10`
  for prerelease tags
- `.github/workflows/release.yml` verify job now does the same for prerelease
  tags after installing nightly and `cargo-fuzz`
- release workflow notes now prepend a curated
  `RELEASE_NOTES_<tag>.md` file when present

### Single-Binary Repository Cleanup

- removed the obsolete `rginx-web`, browser console, node-agent, and related
  control-plane crates from the workspace
- deleted the control-plane compose, Docker, and systemd artifacts that no
  longer match the intended product shape
- simplified CI and release workflows so they only validate, package, and
  publish the `rginx` binary

## Validation Performed

Release-oriented validation passed for `v0.1.3-rc.13` with:

- `bash -n scripts/prepare-release.sh scripts/run-fuzz-smoke.sh scripts/run-fuzz-coverage.sh`
- `./scripts/run-fuzz-smoke.sh --seconds 1 --target proxy_protocol --target certificate_inspect`
- `./scripts/run-fuzz-coverage.sh --target certificate_inspect --format text`
- `cargo metadata --format-version 1`
- `cargo check --workspace --all-targets --message-format short`

## Known Limits

- full fuzzing is still not a mandatory scheduled nightly job; nightly keeps the
  short smoke path behind manual dispatch
- fuzz smoke is currently a prerelease gate, not a stable-release-only
  differentiator
- the HTTP/3 release gate does not include an in-repo nginx comparison harness
