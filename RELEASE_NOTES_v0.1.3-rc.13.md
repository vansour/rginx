# rginx v0.1.3-rc.13

Updated: `2026-04-25`

## Summary

`v0.1.3-rc.13` is a release-discipline and fuzz-hardening candidate.

This release candidate keeps the completed HTTP/3 alignment work intact and
folds the stage 1-10 robustness work into the prerelease path: fuzz targets,
versioned seed corpora, deterministic smoke defaults, and target-scoped
libFuzzer limits are now treated as part of the release story instead of
standalone local tooling.

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

### Documentation Alignment

- README version and release workflow notes now point at `v0.1.3-rc.13`
- README fuzz coverage summary now matches the actual five-target harness
- HTTP/3 release-gate docs now describe prerelease fuzz-smoke consumption

## Validation Performed

Release-oriented validation passed for `v0.1.3-rc.13` with:

- `bash -n scripts/prepare-release.sh scripts/run-fuzz-smoke.sh scripts/run-fuzz-coverage.sh`
- `./scripts/run-fuzz-smoke.sh --seconds 1 --target proxy_protocol --target certificate_inspect`
- `./scripts/run-fuzz-coverage.sh --target certificate_inspect --format text`
- `cargo metadata --format-version 1`
- `(cd fuzz && cargo metadata --format-version 1)`

## Known Limits

- full fuzzing is still not a mandatory scheduled nightly job; nightly keeps the
  short smoke path behind manual dispatch
- fuzz smoke is currently a prerelease gate, not a stable-release-only
  differentiator
- the nginx comparison harness remains external to the normal test matrix and is
  still optional for prerelease preparation
