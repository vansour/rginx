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
    cat >"${path}" <<EOF
$*
EOF
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
'-----BEGIN CERTIFICATE-----
MIIDhDCCAmygAwIBAgIUDlTlXa2cutS5Xq+szBeeAecyaKkwDQYJKoZIhvcNAQEL
BQAwKTESMBAGA1UEAwwJbG9jYWxob3N0MRMwEQYDVQQKDApyZ2lueCBmdXp6MB4X
DTI2MDQyNTA2MTkzOFoXDTI2MDQyNjA2MTkzOFowKTESMBAGA1UEAwwJbG9jYWxo
b3N0MRMwEQYDVQQKDApyZ2lueCBmdXp6MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8A
MIIBCgKCAQEAxuiyo85Kx0barR9khY2RgIBmCDOb4GDtcvjibve7+cuww7Q1YmlO
kDU2k2wuvPVCexzFK2QOxAddhqSYRKwDMhh9J7OeWX2uXU7Q6CVDX3tq3nDupqGF
vs6PiACvjfMQizyYnd1SX/4EGnwGwG20+W/baTuy6E6oR6+I0yhNfyR7fXBb7pq7
69p/FtMzJdwz+bXwpp/6pqDa+MDCZQFavke3OAGJfdciXvUGD9xmX5WjOyYqjVEV
zOGvU6GvJyAgT9+dA3L6iTpMf71jMAPnu3HAb0Y2Lj8hLv3Lg9DANXH/QAU6ThFd
vQ7FLskeCso3cHGcpJjLqKGGe89ngfa7ewIDAQABo4GjMIGgMB0GA1UdDgQWBBSS
Gtp41S51N7kSk1pA4teaSmmGYDAfBgNVHSMEGDAWgBSSGtp41S51N7kSk1pA4tea
SmmGYDAPBgNVHRMBAf8EBTADAQH/MBQGA1UdEQQNMAuCCWxvY2FsaG9zdDA3Bggr
BgEFBQcBAQQrMCkwJwYIKwYBBQUHMAGGG2h0dHA6Ly8xMjcuMC4wLjE6MTkwOTAv
b2NzcDANBgkqhkiG9w0BAQsFAAOCAQEAPDPIkJsuDczRm82y6FefJBdur527dc4H
81NkKvlFkYpu6Q3Wh6MYQNpvDhyo8a3vf1uFMpXfK5AN7kZa2iw/cows8oJEhy6E
NOwTgtkagJyXnTfh+Y2saNri2L7baUG4x8Rv2N5m/rXfT3F05Om0Rs1zCDg7vM5y
3bQnuD0I8ot+Fi3ca4PjvGm2VEkHcQCTdEMvFBbEzWVP6Jw35S9c6mR7h/bL0yGn
+OaqKfJFcieoPgcb6ItQRmKnxcyBkGyEcTvDDns3/sfALKlTzivC8bD34BjJXsV9
VEwb+H+srzKmW5RhW7j3ye5vizTKl48o5G5a06IsuCtztDypmD+9Lw==
-----END CERTIFICATE-----'
write_text_seed \
    "${FUZZ_DIR}/corpus/certificate_inspect/invalid_pem.seed" \
'-----BEGIN CERTIFICATE-----
Zm9vYmFy
-----END CERTIFICATE-----'

write_text_seed \
    "${FUZZ_DIR}/corpus/ocsp_responder_discovery/aia_pem.seed" \
'-----BEGIN CERTIFICATE-----
MIIDhDCCAmygAwIBAgIUDlTlXa2cutS5Xq+szBeeAecyaKkwDQYJKoZIhvcNAQEL
BQAwKTESMBAGA1UEAwwJbG9jYWxob3N0MRMwEQYDVQQKDApyZ2lueCBmdXp6MB4X
DTI2MDQyNTA2MTkzOFoXDTI2MDQyNjA2MTkzOFowKTESMBAGA1UEAwwJbG9jYWxo
b3N0MRMwEQYDVQQKDApyZ2lueCBmdXp6MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8A
MIIBCgKCAQEAxuiyo85Kx0barR9khY2RgIBmCDOb4GDtcvjibve7+cuww7Q1YmlO
kDU2k2wuvPVCexzFK2QOxAddhqSYRKwDMhh9J7OeWX2uXU7Q6CVDX3tq3nDupqGF
vs6PiACvjfMQizyYnd1SX/4EGnwGwG20+W/baTuy6E6oR6+I0yhNfyR7fXBb7pq7
69p/FtMzJdwz+bXwpp/6pqDa+MDCZQFavke3OAGJfdciXvUGD9xmX5WjOyYqjVEV
zOGvU6GvJyAgT9+dA3L6iTpMf71jMAPnu3HAb0Y2Lj8hLv3Lg9DANXH/QAU6ThFd
vQ7FLskeCso3cHGcpJjLqKGGe89ngfa7ewIDAQABo4GjMIGgMB0GA1UdDgQWBBSS
Gtp41S51N7kSk1pA4teaSmmGYDAfBgNVHSMEGDAWgBSSGtp41S51N7kSk1pA4tea
SmmGYDAPBgNVHRMBAf8EBTADAQH/MBQGA1UdEQQNMAuCCWxvY2FsaG9zdDA3Bggr
BgEFBQcBAQQrMCkwJwYIKwYBBQUHMAGGG2h0dHA6Ly8xMjcuMC4wLjE6MTkwOTAv
b2NzcDANBgkqhkiG9w0BAQsFAAOCAQEAPDPIkJsuDczRm82y6FefJBdur527dc4H
81NkKvlFkYpu6Q3Wh6MYQNpvDhyo8a3vf1uFMpXfK5AN7kZa2iw/cows8oJEhy6E
NOwTgtkagJyXnTfh+Y2saNri2L7baUG4x8Rv2N5m/rXfT3F05Om0Rs1zCDg7vM5y
3bQnuD0I8ot+Fi3ca4PjvGm2VEkHcQCTdEMvFBbEzWVP6Jw35S9c6mR7h/bL0yGn
+OaqKfJFcieoPgcb6ItQRmKnxcyBkGyEcTvDDns3/sfALKlTzivC8bD34BjJXsV9
VEwb+H+srzKmW5RhW7j3ye5vizTKl48o5G5a06IsuCtztDypmD+9Lw==
-----END CERTIFICATE-----'
write_text_seed \
    "${FUZZ_DIR}/corpus/ocsp_responder_discovery/no_pem_items.seed" \
'not a pem certificate'

log "seed corpus refreshed under ${FUZZ_DIR}/corpus"
