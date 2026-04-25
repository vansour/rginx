#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
FUZZ_DIR="${ROOT_DIR}/fuzz"

usage() {
    cat <<'EOF'
Usage: refresh-fuzz-seeds.sh

Regenerate the curated seed corpus under fuzz/corpus/.
EOF
}

log() {
    printf '[fuzz-seeds] %s\n' "$*"
}

write_text_seed() {
    local path="$1"
    shift
    printf '%s\n' "$*" >"${path}"
}

write_binary_seed() {
    local path="$1"
    shift
    printf '%b' "$*" >"${path}"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        *)
            printf '[fuzz-seeds] error: unknown option: %s\n' "$1" >&2
            exit 1
            ;;
    esac
done

mkdir -p \
    "${FUZZ_DIR}/corpus/proxy_protocol" \
    "${FUZZ_DIR}/corpus/config_preprocess" \
    "${FUZZ_DIR}/corpus/ocsp_response" \
    "${FUZZ_DIR}/corpus/certificate_inspect" \
    "${FUZZ_DIR}/corpus/ocsp_responder_discovery"

SELF_SIGNED_PEM='-----BEGIN CERTIFICATE-----
MIIDlDCCAnygAwIBAgIUCCWjaIn/zNv2xGSDzkVcuIEijCUwDQYJKoZIhvcNAQEL
BQAwKTESMBAGA1UEAwwJbG9jYWxob3N0MRMwEQYDVQQKDApyZ2lueCBmdXp6MB4X
DTI2MDQyNTA3NTA1MloXDTM2MDQyMjA3NTA1MlowKTESMBAGA1UEAwwJbG9jYWxo
b3N0MRMwEQYDVQQKDApyZ2lueCBmdXp6MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8A
MIIBCgKCAQEAqA/tteNt807luOPrD0s5hGphnK37PjndsaF67I0VbGVp5ouqeqtx
NEDJB4MygVsQi+AIJKP34zKrtrtSe5yZlibtwNt/kE3xxS9LHlVMB4xYAVmyGsRs
oJ0kiXYPK6Y7qWwj+xnmPCbRrNi79h1V2yf9EF2n0+TFn4yuXYv/o9QRV+Pkmgcd
UHPYFbynPmW78MULW37wMYu5+g3Hkb6/QFTSBDjB6gAx3BojW3z3yyBBri0G1aSA
jmevSqVbTCK7jlUe16Ex53cCLBmc76WR7/lMlhXLSS/M94a1SU1b1y0IPrlkTtlE
lCXEKOB956T043p7d65cB8YkQHl6jJj8IwIDAQABo4GzMIGwMB0GA1UdDgQWBBQT
cnrhABE3wlceAB8Q0zu9NK48JDAfBgNVHSMEGDAWgBQTcnrhABE3wlceAB8Q0zu9
NK48JDAUBgNVHREEDTALgglsb2NhbGhvc3QwDwYDVR0TAQH/BAUwAwEB/zAOBgNV
HQ8BAf8EBAMCAqQwNwYIKwYBBQUHAQEEKzApMCcGCCsGAQUFBzABhhtodHRwOi8v
MTI3LjAuMC4xOjE5MDkwL29jc3AwDQYJKoZIhvcNAQELBQADggEBABKP4xzaAFob
PvqnwSmuDR17EZdjGjaX9Hxawjo/ut0oap6FCWiDw2ddcTmMO0TZP0RxR7IIGaH4
K2ZzqN2zRHVNMzn/nA1myy9D0TpMoBTkdhgW//2PK+eALOxlyN1Bu+zo/gsF0RyZ
0Yai3UnxkExR4NROd3G+m0R9FHGa74eKDGIczyDf5EJ/jPZZzn2BRwhRYceXy0Bj
VLLdvg15A1gaJYlQiYfsilviSn9KQzkosgtfn9fbm3fOOQQTyqKvXouHbMNRELOJ
thvb6fUsjTJ/yrfbjFS4leYfJq+TmwxXaAtBvfNQh2FEhL5qn7YXfNuRFW2VKzW6
nkPpNiaz2iU=
-----END CERTIFICATE-----'

write_binary_seed \
    "${FUZZ_DIR}/corpus/proxy_protocol/trusted_tcp4.seed" \
    '1PROXY TCP4 198.51.100.9 203.0.113.10 12345 443\r\n'
write_binary_seed \
    "${FUZZ_DIR}/corpus/proxy_protocol/untrusted_tcp6.seed" \
    '0PROXY TCP6 2001:db8::10 2001:db8::20 65535 443\r\n'
write_binary_seed \
    "${FUZZ_DIR}/corpus/proxy_protocol/unknown.seed" \
    '1PROXY UNKNOWN\r\n'
write_binary_seed \
    "${FUZZ_DIR}/corpus/proxy_protocol/invalid_prefix.seed" \
    '1BROKEN\r\n'

write_text_seed \
    "${FUZZ_DIR}/corpus/config_preprocess/minimal_return.seed" \
'Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 2,
    ),
    server: ServerConfig(
        listen: "127.0.0.1:18080",
    ),
    upstreams: [],
    locations: [
        LocationConfig(
            matcher: Exact("/"),
            handler: Return(
                status: 200,
                location: "",
                body: Some("ok\n"),
            ),
        ),
    ],
)'

write_text_seed \
    "${FUZZ_DIR}/corpus/config_preprocess/env_defaults.seed" \
'Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 2,
    ),
    server: ServerConfig(
        listen: "${rginx_fuzz_listen:-127.0.0.1:19090}",
    ),
    upstreams: [],
    locations: [
        LocationConfig(
            matcher: Exact("/"),
            handler: Return(
                status: 200,
                location: "",
                body: Some("$${literal}:${rginx_fuzz_body:-hello}"),
            ),
        ),
    ],
)'

write_text_seed \
    "${FUZZ_DIR}/corpus/config_preprocess/include_glob.seed" \
'Config(
    runtime: RuntimeConfig(
        shutdown_timeout_secs: 2,
    ),
    server: ServerConfig(
        listen: "127.0.0.1:18081",
    ),
    upstreams: [],
    locations: [
        LocationConfig(
            matcher: Exact("/"),
            handler: Return(
                status: 200,
                location: "",
                body: Some("root\n"),
            ),
        ),
    ],
    servers: [
        // @include "conf.d/*.ron"
    ],
)'

write_binary_seed \
    "${FUZZ_DIR}/corpus/ocsp_response/status_success_no_body.seed" \
    '\x30\x03\x0a\x01\x00'
write_binary_seed \
    "${FUZZ_DIR}/corpus/ocsp_response/status_unauthorized.seed" \
    '\x30\x03\x0a\x01\x06'
write_binary_seed \
    "${FUZZ_DIR}/corpus/ocsp_response/basic_response_empty_octet.seed" \
    '\x30\x16\x0a\x01\x00\xa0\x11\x30\x0f\x06\x09\x2b\x06\x01\x05\x05\x07\x30\x01\x01\x04\x02\x30\x00'
write_binary_seed \
    "${FUZZ_DIR}/corpus/ocsp_response/unsupported_response_type.seed" \
    '\x30\x16\x0a\x01\x00\xa0\x11\x30\x0f\x06\x09\x2b\x06\x01\x05\x05\x07\x30\x01\x02\x04\x02\x30\x00'

write_text_seed \
    "${FUZZ_DIR}/corpus/certificate_inspect/self_signed_pem.seed" \
"${SELF_SIGNED_PEM}"
write_text_seed \
    "${FUZZ_DIR}/corpus/certificate_inspect/invalid_pem.seed" \
'-----BEGIN CERTIFICATE-----
Zm9vYmFy
-----END CERTIFICATE-----'

write_text_seed \
    "${FUZZ_DIR}/corpus/ocsp_responder_discovery/aia_pem.seed" \
"${SELF_SIGNED_PEM}"
write_text_seed \
    "${FUZZ_DIR}/corpus/ocsp_responder_discovery/no_pem_items.seed" \
'not a pem certificate'

log "seed corpus refreshed under ${FUZZ_DIR}/corpus"
