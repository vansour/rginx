#!/bin/sh
set -eu

mode="${1:-api}"
shift || true

case "${mode}" in
    api)
        exec /usr/local/bin/rginx-control-api "$@"
        ;;
    worker)
        exec /usr/local/bin/rginx-control-worker "$@"
        ;;
    *)
        echo "unsupported rginx-control mode: ${mode}" >&2
        exit 64
        ;;
esac
