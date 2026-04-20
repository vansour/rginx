#!/usr/bin/env bash
set -euo pipefail

SCRIPT_SOURCE="${BASH_SOURCE[0]:-$0}"
SCRIPT_DIR="$(cd "$(dirname "${SCRIPT_SOURCE}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

PROJECT_NAME="${COMPOSE_PROJECT_NAME:-rginx-web-smoke}"
WEB_IMAGE_TAG="${RGINX_WEB_IMAGE_TAG:-smoke}"
API_PUBLISH="${RGINX_CONTROL_API_PUBLISH:-127.0.0.1:18080}"
POSTGRES_PUBLISH="${RGINX_CONTROL_POSTGRES_PUBLISH:-127.0.0.1:15432}"
DRAGONFLY_PUBLISH="${RGINX_CONTROL_DRAGONFLY_PUBLISH:-127.0.0.1:16379}"
NODE_ID="${RGINX_CONTROL_SMOKE_NODE_ID:-edge-dns-smoke-01}"
CLUSTER_ID="${RGINX_CONTROL_SMOKE_CLUSTER_ID:-cluster-mainland}"
NODE_ADVERTISE_ADDR="${RGINX_CONTROL_SMOKE_NODE_ADVERTISE_ADDR:-127.0.0.1:9443}"
NODE_DNS_UDP_ADDR="${RGINX_CONTROL_SMOKE_NODE_DNS_UDP_ADDR:-127.0.0.1:19053}"
NODE_DNS_TCP_ADDR="${RGINX_CONTROL_SMOKE_NODE_DNS_TCP_ADDR:-127.0.0.1:19054}"
DNS_ZONE="${RGINX_CONTROL_SMOKE_DNS_ZONE:-smoke.example.test}"
DNS_QNAME="${RGINX_CONTROL_SMOKE_DNS_QNAME:-www.smoke.example.test}"
DNS_EXPECTED_IP="${RGINX_CONTROL_SMOKE_DNS_EXPECTED_IP:-127.0.0.1}"

TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/rginx-control-plane-smoke.XXXXXX")"
ADMIN_SOCKET_PATH="${TMP_ROOT}/admin.sock"
NODE_AGENT_LOG="${TMP_ROOT}/node-agent.log"
FAKE_ADMIN_LOG="${TMP_ROOT}/fake-admin.log"
NODE_AGENT_CONFIG_PATH="${TMP_ROOT}/node-agent.ron"
NODE_AGENT_BACKUP_DIR="${TMP_ROOT}/backups"
NODE_AGENT_STAGING_DIR="${TMP_ROOT}/staging"

FAKE_ADMIN_PID=""
NODE_AGENT_PID=""
AUTH_TOKEN=""

log() {
    printf '[control-plane-smoke] %s\n' "$*"
}

die() {
    printf '[control-plane-smoke] error: %s\n' "$*" >&2
    exit 1
}

have() {
    command -v "$1" >/dev/null 2>&1
}

resolve_http_host_port() {
    local bind="$1"
    local host="${bind%:*}"
    local port="${bind##*:}"

    if [[ "${host}" == "${bind}" ]]; then
        host="127.0.0.1"
    elif [[ "${host}" == "0.0.0.0" ]]; then
        host="127.0.0.1"
    fi

    printf '%s:%s\n' "${host}" "${port}"
}

json_field() {
    local path="$1"
    local payload
    payload="$(cat)"
    JSON_PAYLOAD="${payload}" python3 - "$path" <<'PY'
import json
import os
import sys

path = [part for part in sys.argv[1].split(".") if part]
value = json.loads(os.environ["JSON_PAYLOAD"])

for part in path:
    if part.isdigit():
        value = value[int(part)]
    else:
        value = value[part]

if value is None:
    sys.exit(0)
if isinstance(value, str):
    sys.stdout.write(value)
else:
    sys.stdout.write(json.dumps(value, separators=(",", ":")))
PY
}

api_get() {
    local path="$1"
    curl -fsS \
        -H "Authorization: Bearer ${AUTH_TOKEN}" \
        "${API_BASE_URL}${path}"
}

api_post_json() {
    local path="$1"
    local body="$2"
    curl -fsS \
        -H "Authorization: Bearer ${AUTH_TOKEN}" \
        -H "Content-Type: application/json" \
        -d "${body}" \
        "${API_BASE_URL}${path}"
}

api_post_empty() {
    local path="$1"
    curl -fsS \
        -X POST \
        -H "Authorization: Bearer ${AUTH_TOKEN}" \
        "${API_BASE_URL}${path}"
}

fetch_node_detail() {
    curl -fsS \
        -H "Authorization: Bearer ${AUTH_TOKEN}" \
        "${API_BASE_URL}/api/v1/nodes/${NODE_ID}" 2>/dev/null || true
}

ensure_background_processes_alive() {
    if [[ -n "${FAKE_ADMIN_PID}" ]] && ! kill -0 "${FAKE_ADMIN_PID}" 2>/dev/null; then
        die "fake admin socket server exited unexpectedly"
    fi
    if [[ -n "${NODE_AGENT_PID}" ]] && ! kill -0 "${NODE_AGENT_PID}" 2>/dev/null; then
        die "node agent exited unexpectedly"
    fi
}

wait_for_http_health() {
    local response=""
    log "waiting for health endpoint ${HEALTH_URL}"
    for _attempt in $(seq 1 60); do
        response="$(curl -fsS "${HEALTH_URL}" 2>/dev/null || true)"
        if [[ -n "${response}" ]]; then
            break
        fi
        sleep 2
    done

    [[ -n "${response:-}" ]] || die "health endpoint never became ready"
    [[ "${response}" == *'"service":"rginx-web"'* ]] || die "unexpected health response: ${response}"
    [[ "${response}" == *'"status":"ok"'* ]] || die "unexpected health response: ${response}"
}

wait_for_fake_admin_socket() {
    for _attempt in $(seq 1 30); do
        if [[ -S "${ADMIN_SOCKET_PATH}" ]]; then
            return 0
        fi
        ensure_background_processes_alive
        sleep 1
    done
    die "fake admin socket never became ready at ${ADMIN_SOCKET_PATH}"
}

wait_for_node_snapshot() {
    local response=""
    for _attempt in $(seq 1 60); do
        ensure_background_processes_alive
        response="$(fetch_node_detail)"
        if [[ -n "${response}" ]] && JSON_PAYLOAD="${response}" python3 - "${NODE_ID}" <<'PY'
import json
import os
import sys

node_id = sys.argv[1]
payload = json.loads(os.environ["JSON_PAYLOAD"])

if payload["node"]["node_id"] != node_id:
    raise SystemExit(1)
if payload["latest_snapshot"] is None:
    raise SystemExit(1)
PY
        then
            printf '%s' "${response}"
            return 0
        fi
        sleep 1
    done
    die "node ${NODE_ID} never uploaded a snapshot"
}

wait_for_node_dns_revision() {
    local revision_id="$1"
    local response=""
    for _attempt in $(seq 1 60); do
        ensure_background_processes_alive
        response="$(fetch_node_detail)"
        if [[ -n "${response}" ]] && JSON_PAYLOAD="${response}" python3 - "${revision_id}" "${NODE_DNS_UDP_ADDR}" "${NODE_DNS_TCP_ADDR}" <<'PY'
import json
import os
import sys

revision_id = sys.argv[1]
expected_udp = sys.argv[2]
expected_tcp = sys.argv[3]
payload = json.loads(os.environ["JSON_PAYLOAD"])
snapshot = payload.get("latest_snapshot")
if snapshot is None:
    raise SystemExit(1)
status = snapshot.get("status") or {}
dns = status.get("dns") or {}
if dns.get("published_revision_id") != revision_id:
    raise SystemExit(1)
if dns.get("udp_bind_addr") != expected_udp:
    raise SystemExit(1)
if dns.get("tcp_bind_addr") != expected_tcp:
    raise SystemExit(1)
PY
        then
            printf '%s' "${response}"
            return 0
        fi
        sleep 1
    done
    die "node ${NODE_ID} never reported dns revision ${revision_id}"
}

wait_for_node_query_total() {
    local min_queries="$1"
    local response=""
    for _attempt in $(seq 1 60); do
        ensure_background_processes_alive
        response="$(fetch_node_detail)"
        if [[ -n "${response}" ]] && JSON_PAYLOAD="${response}" python3 - "${min_queries}" <<'PY'
import json
import os
import sys

min_queries = int(sys.argv[1])
payload = json.loads(os.environ["JSON_PAYLOAD"])
snapshot = payload.get("latest_snapshot")
if snapshot is None:
    raise SystemExit(1)
status = snapshot.get("status") or {}
dns = status.get("dns") or {}
if int(dns.get("query_total", 0)) < min_queries:
    raise SystemExit(1)
PY
        then
            printf '%s' "${response}"
            return 0
        fi
        sleep 1
    done
    die "node ${NODE_ID} never reported dns query_total >= ${min_queries}"
}

wait_for_dns_deployment_status() {
    local deployment_id="$1"
    local expected_status="$2"
    local response=""
    for _attempt in $(seq 1 90); do
        response="$(api_get "/api/v1/dns/deployments/${deployment_id}" 2>/dev/null || true)"
        if [[ -n "${response}" ]] && JSON_PAYLOAD="${response}" python3 - "${expected_status}" <<'PY'
import json
import os
import sys

expected_status = sys.argv[1]
payload = json.loads(os.environ["JSON_PAYLOAD"])
deployment = payload.get("deployment") or {}
if deployment.get("status") != expected_status:
    raise SystemExit(1)
targets = payload.get("targets") or []
if expected_status == "succeeded" and not all(target.get("state") == "succeeded" for target in targets):
    raise SystemExit(1)
PY
        then
            printf '%s' "${response}"
            return 0
        fi
        sleep 1
    done
    die "dns deployment ${deployment_id} never reached status ${expected_status}"
}

wait_for_dashboard_dns_deployment_active() {
    local deployment_id="$1"
    local revision_id="$2"
    local response=""
    for _attempt in $(seq 1 30); do
        response="$(api_get "/api/v1/dashboard" 2>/dev/null || true)"
        if [[ -n "${response}" ]] && JSON_PAYLOAD="${response}" python3 - "${deployment_id}" "${revision_id}" <<'PY'
import json
import os
import sys

deployment_id = sys.argv[1]
revision_id = sys.argv[2]
payload = json.loads(os.environ["JSON_PAYLOAD"])
items = payload.get("recent_dns_deployments") or []
match = next((item for item in items if item.get("deployment_id") == deployment_id), None)
if match is None:
    raise SystemExit(1)
if match.get("revision_id") != revision_id:
    raise SystemExit(1)
if int(payload.get("active_dns_deployments", 0)) < 1:
    raise SystemExit(1)
PY
        then
            printf '%s' "${response}"
            return 0
        fi
        sleep 1
    done
    die "dashboard never exposed active dns deployment ${deployment_id}"
}

wait_for_dns_deployment_metrics() {
    local deployment_id="$1"
    local revision_id="$2"
    local response=""
    for _attempt in $(seq 1 30); do
        response="$(curl -fsS "${API_BASE_URL}/metrics" 2>/dev/null || true)"
        if [[ -n "${response}" ]] && METRICS_PAYLOAD="${response}" python3 - "${deployment_id}" "${revision_id}" <<'PY'
import os
import sys

deployment_id = sys.argv[1]
revision_id = sys.argv[2]
payload = os.environ["METRICS_PAYLOAD"]
info_lines = [
    line for line in payload.splitlines()
    if line.startswith("rginx_control_dns_deployment_info{")
]
target_lines = [
    line for line in payload.splitlines()
    if line.startswith("rginx_control_dns_deployment_targets{")
]
if not any(f'deployment_id="{deployment_id}"' in line and f'revision_id="{revision_id}"' in line for line in info_lines):
    raise SystemExit(1)
if not any(f'deployment_id="{deployment_id}"' in line and 'state="succeeded"' in line and line.endswith(" 1") for line in target_lines):
    raise SystemExit(1)
PY
        then
            printf '%s' "${response}"
            return 0
        fi
        sleep 1
    done
    die "metrics endpoint never exposed dns deployment ${deployment_id}"
}

fetch_dns_deployment_sse_event() {
    local deployment_id="$1"
    python3 - "${API_BASE_URL}" "${AUTH_TOKEN}" "${deployment_id}" <<'PY'
import json
import sys
import urllib.request

base_url = sys.argv[1]
token = sys.argv[2]
deployment_id = sys.argv[3]
request = urllib.request.Request(
    f"{base_url}/api/v1/events?dns_deployment_id={deployment_id}",
    headers={
        "Authorization": f"Bearer {token}",
        "Accept": "text/event-stream",
    },
)

with urllib.request.urlopen(request, timeout=10) as response:
    event_name = None
    data_lines = []
    while True:
        raw_line = response.readline()
        if not raw_line:
            break
        line = raw_line.decode("utf-8").strip()
        if not line:
            if event_name is None and not data_lines:
                continue
            sys.stdout.write(
                json.dumps(
                    {"event": event_name, "data": json.loads("\n".join(data_lines))},
                    separators=(",", ":"),
                )
            )
            raise SystemExit(0)
        if line.startswith(":"):
            continue
        if line.startswith("event:"):
            event_name = line.split(":", 1)[1].strip()
        elif line.startswith("data:"):
            data_lines.append(line.split(":", 1)[1].strip())

raise SystemExit(1)
PY
}

run_dns_query() {
    local transport="$1"
    local bind_addr="$2"
    local qname="$3"
    local expected_ip="$4"
    local host="${bind_addr%:*}"
    local port="${bind_addr##*:}"

    python3 - "${transport}" "${host}" "${port}" "${qname}" "${expected_ip}" <<'PY'
import random
import socket
import struct
import sys

transport = sys.argv[1]
host = sys.argv[2]
port = int(sys.argv[3])
qname = sys.argv[4].rstrip(".")
expected_ip = sys.argv[5]

query_id = random.randint(0, 0xFFFF)

def encode_name(name: str) -> bytes:
    return b"".join(len(label).to_bytes(1, "big") + label.encode("ascii") for label in name.split(".")) + b"\x00"

def read_name(buffer: bytes, offset: int):
    labels = []
    jumped = False
    next_offset = offset
    while True:
        length = buffer[offset]
        if length & 0xC0 == 0xC0:
            pointer = ((length & 0x3F) << 8) | buffer[offset + 1]
            if not jumped:
                next_offset = offset + 2
                jumped = True
            offset = pointer
            continue
        if length == 0:
            offset += 1
            if not jumped:
                next_offset = offset
            break
        offset += 1
        labels.append(buffer[offset : offset + length].decode("ascii"))
        offset += length
        if not jumped:
            next_offset = offset
    return ".".join(labels), next_offset

question = encode_name(qname) + struct.pack("!HH", 1, 1)
message = struct.pack("!HHHHHH", query_id, 0x0100, 1, 0, 0, 0) + question

if transport == "udp":
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.settimeout(2.0)
        sock.sendto(message, (host, port))
        response, _ = sock.recvfrom(4096)
elif transport == "tcp":
    with socket.create_connection((host, port), timeout=2.0) as sock:
        sock.settimeout(2.0)
        sock.sendall(struct.pack("!H", len(message)) + message)
        length_data = sock.recv(2)
        if len(length_data) != 2:
            raise SystemExit("short tcp dns response length")
        remaining = struct.unpack("!H", length_data)[0]
        chunks = bytearray()
        while len(chunks) < remaining:
            chunk = sock.recv(remaining - len(chunks))
            if not chunk:
                raise SystemExit("tcp dns response closed early")
            chunks.extend(chunk)
        response = bytes(chunks)
else:
    raise SystemExit(f"unsupported transport {transport}")

response_id, flags, qdcount, ancount, _, _ = struct.unpack("!HHHHHH", response[:12])
rcode = flags & 0x000F
if response_id != query_id:
    raise SystemExit(f"dns response id mismatch: expected {query_id}, got {response_id}")
if rcode != 0:
    raise SystemExit(f"dns query returned rcode {rcode}")
if ancount == 0:
    raise SystemExit("dns query returned no answers")

offset = 12
for _ in range(qdcount):
    _, offset = read_name(response, offset)
    offset += 4

answers = []
for _ in range(ancount):
    _, offset = read_name(response, offset)
    rtype, rclass, ttl, rdlength = struct.unpack("!HHIH", response[offset : offset + 10])
    offset += 10
    rdata = response[offset : offset + rdlength]
    offset += rdlength
    if rclass != 1:
        continue
    if rtype == 1 and rdlength == 4:
        answers.append(socket.inet_ntoa(rdata))
    elif rtype == 28 and rdlength == 16:
        answers.append(socket.inet_ntop(socket.AF_INET6, rdata))

if expected_ip not in answers:
    raise SystemExit(f"dns answers {answers!r} did not contain expected {expected_ip}")
PY
}

run_dns_nxdomain_query() {
    local transport="$1"
    local bind_addr="$2"
    local qname="$3"
    local host="${bind_addr%:*}"
    local port="${bind_addr##*:}"

    python3 - "${transport}" "${host}" "${port}" "${qname}" <<'PY'
import random
import socket
import struct
import sys

transport = sys.argv[1]
host = sys.argv[2]
port = int(sys.argv[3])
qname = sys.argv[4].rstrip(".")
query_id = random.randint(0, 0xFFFF)

def encode_name(name: str) -> bytes:
    return b"".join(len(label).to_bytes(1, "big") + label.encode("ascii") for label in name.split(".")) + b"\x00"

question = encode_name(qname) + struct.pack("!HH", 1, 1)
message = struct.pack("!HHHHHH", query_id, 0x0100, 1, 0, 0, 0) + question

if transport == "udp":
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.settimeout(2.0)
        sock.sendto(message, (host, port))
        response, _ = sock.recvfrom(4096)
elif transport == "tcp":
    with socket.create_connection((host, port), timeout=2.0) as sock:
        sock.settimeout(2.0)
        sock.sendall(struct.pack("!H", len(message)) + message)
        length_data = sock.recv(2)
        if len(length_data) != 2:
            raise SystemExit("short tcp dns response length")
        remaining = struct.unpack("!H", length_data)[0]
        chunks = bytearray()
        while len(chunks) < remaining:
            chunk = sock.recv(remaining - len(chunks))
            if not chunk:
                raise SystemExit("tcp dns response closed early")
            chunks.extend(chunk)
        response = bytes(chunks)
else:
    raise SystemExit(f"unsupported transport {transport}")

response_id, flags, _, ancount, _, _ = struct.unpack("!HHHHHH", response[:12])
rcode = flags & 0x000F
if response_id != query_id:
    raise SystemExit(f"dns response id mismatch: expected {query_id}, got {response_id}")
if rcode != 3:
    raise SystemExit(f"dns query returned rcode {rcode}, expected 3")
if ancount != 0:
    raise SystemExit(f"dns query returned unexpected answers count {ancount}")
PY
}

start_fake_admin_socket() {
    log "starting fake admin socket at ${ADMIN_SOCKET_PATH}"
    python3 - "${ADMIN_SOCKET_PATH}" >"${FAKE_ADMIN_LOG}" 2>&1 <<'PY' &
import json
import os
import signal
import socket
import sys
import time

socket_path = sys.argv[1]
running = True

def handle_signal(_signum, _frame):
    global running
    running = False

signal.signal(signal.SIGTERM, handle_signal)
signal.signal(signal.SIGINT, handle_signal)

if os.path.exists(socket_path):
    os.unlink(socket_path)

server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(socket_path)
server.listen(16)
server.settimeout(0.5)

while running:
    try:
        conn, _ = server.accept()
    except socket.timeout:
        continue
    except OSError:
        break

    with conn:
        request = b""
        while not request.endswith(b"\n"):
            chunk = conn.recv(4096)
            if not chunk:
                break
            request += chunk

        now_ms = int(time.time() * 1000)
        payload = {
            "schema_version": 1,
            "snapshot_version": 9001,
            "captured_at_unix_ms": now_ms,
            "pid": 4242,
            "binary_version": "smoke-admin",
            "included_modules": ["http", "dns"],
            "status": {
                "revision": 42,
                "listeners": [],
                "active_connections": 0,
            },
            "counters": {},
            "traffic": {},
            "peer_health": {},
            "upstreams": {},
        }
        response = json.dumps({"type": "Snapshot", "data": payload}, separators=(",", ":")).encode("utf-8") + b"\n"
        conn.sendall(response)

try:
    server.close()
finally:
    if os.path.exists(socket_path):
        os.unlink(socket_path)
PY
    FAKE_ADMIN_PID="$!"
}

start_node_agent() {
    mkdir -p "${NODE_AGENT_BACKUP_DIR}" "${NODE_AGENT_STAGING_DIR}"
    log "starting node agent ${NODE_ID}"
    env \
        RGINX_NODE_ID="${NODE_ID}" \
        RGINX_CLUSTER_ID="${CLUSTER_ID}" \
        RGINX_NODE_ADVERTISE_ADDR="${NODE_ADVERTISE_ADDR}" \
        RGINX_NODE_ROLE="edge" \
        RGINX_NODE_RUNNING_VERSION="smoke-agent" \
        RGINX_NODE_LIFECYCLE_STATE="online" \
        RGINX_CONTROL_PLANE_ORIGIN="${API_ORIGIN}" \
        RGINX_CONTROL_AGENT_SHARED_TOKEN="change-me-for-node-agent" \
        RGINX_ADMIN_SOCKET="${ADMIN_SOCKET_PATH}" \
        RGINX_NODE_AGENT_HEARTBEAT_SECS="1" \
        RGINX_NODE_AGENT_TASK_POLL_SECS="1" \
        RGINX_NODE_AGENT_REQUEST_TIMEOUT_SECS="5" \
        RGINX_NODE_DNS_UDP_ADDR="${NODE_DNS_UDP_ADDR}" \
        RGINX_NODE_DNS_TCP_ADDR="${NODE_DNS_TCP_ADDR}" \
        RGINX_NODE_BINARY="/bin/true" \
        RGINX_NODE_CONFIG_PATH="${NODE_AGENT_CONFIG_PATH}" \
        RGINX_NODE_CONFIG_BACKUP_DIR="${NODE_AGENT_BACKUP_DIR}" \
        RGINX_NODE_CONFIG_STAGING_DIR="${NODE_AGENT_STAGING_DIR}" \
        "${ROOT_DIR}/target/debug/rginx-node-agent" >"${NODE_AGENT_LOG}" 2>&1 &
    NODE_AGENT_PID="$!"
}

login_control_plane() {
    local response
    response="$(curl -fsS \
        -H "Content-Type: application/json" \
        -d '{"username":"admin","password":"admin"}' \
        "${API_BASE_URL}/api/v1/auth/login")"
    AUTH_TOKEN="$(printf '%s' "${response}" | json_field "token")"
    [[ -n "${AUTH_TOKEN}" ]] || die "failed to obtain control-plane auth token"
}

cleanup() {
    local exit_code="${1:-0}"

    if [[ -n "${NODE_AGENT_PID}" ]]; then
        kill "${NODE_AGENT_PID}" >/dev/null 2>&1 || true
        wait "${NODE_AGENT_PID}" >/dev/null 2>&1 || true
    fi
    if [[ -n "${FAKE_ADMIN_PID}" ]]; then
        kill "${FAKE_ADMIN_PID}" >/dev/null 2>&1 || true
        wait "${FAKE_ADMIN_PID}" >/dev/null 2>&1 || true
    fi

    if [[ "${exit_code}" -ne 0 ]]; then
        log "smoke test failed; dumping compose state"
        docker compose -p "${PROJECT_NAME}" -f "${ROOT_DIR}/compose.yaml" ps || true
        docker compose -p "${PROJECT_NAME}" -f "${ROOT_DIR}/compose.yaml" logs --no-color --tail=200 || true
        if [[ -f "${NODE_AGENT_LOG}" ]]; then
            log "node-agent log tail"
            tail -n 200 "${NODE_AGENT_LOG}" || true
        fi
        if [[ -f "${FAKE_ADMIN_LOG}" ]]; then
            log "fake-admin log tail"
            tail -n 200 "${FAKE_ADMIN_LOG}" || true
        fi
    fi

    docker compose -p "${PROJECT_NAME}" -f "${ROOT_DIR}/compose.yaml" down -v --remove-orphans >/dev/null 2>&1 || true
    rm -rf "${TMP_ROOT}"
}

trap 'cleanup $?' EXIT

have cargo || die "cargo is required"
have curl || die "curl is required"
have docker || die "docker is required"
have python3 || die "python3 is required"

cd "${ROOT_DIR}"

HTTP_HOST_PORT="$(resolve_http_host_port "${API_PUBLISH}")"
API_ORIGIN="http://${HTTP_HOST_PORT}"
API_BASE_URL="${API_ORIGIN}"
HEALTH_URL="${API_ORIGIN}/healthz"
ROOT_URL="${API_ORIGIN}/"
CONSOLE_JS_URL="${API_ORIGIN}/console.js"

log "building local node-agent binary"
cargo build --locked -p rginx-node-agent >/dev/null

log "starting smoke stack with project ${PROJECT_NAME}"
COMPOSE_PROJECT_NAME="${PROJECT_NAME}" \
RGINX_WEB_IMAGE_TAG="${WEB_IMAGE_TAG}" \
RGINX_CONTROL_API_PUBLISH="${API_PUBLISH}" \
RGINX_CONTROL_POSTGRES_PUBLISH="${POSTGRES_PUBLISH}" \
RGINX_CONTROL_DRAGONFLY_PUBLISH="${DRAGONFLY_PUBLISH}" \
docker compose -p "${PROJECT_NAME}" -f "${ROOT_DIR}/compose.yaml" up -d --build --remove-orphans

wait_for_http_health

log "verifying bundled console assets"
curl -fsSI "${ROOT_URL}" >/dev/null
curl -fsSI "${CONSOLE_JS_URL}" >/dev/null

start_fake_admin_socket
wait_for_fake_admin_socket
start_node_agent
sleep 1
ensure_background_processes_alive

login_control_plane

log "waiting for node ${NODE_ID} registration and initial snapshot"
wait_for_node_snapshot >/dev/null

log "creating dns draft targeting node ${NODE_ID}"
CREATE_DRAFT_RESPONSE="$(api_post_json \
    "/api/v1/dns/revisions/drafts" \
    "$(cat <<JSON
{"cluster_id":"${CLUSTER_ID}","title":"dns-smoke-e2e","summary":"control-plane dns smoke e2e","base_revision_id":null,"plan":{"cluster_id":"${CLUSTER_ID}","zones":[{"zone_id":"zone-smoke","zone_name":"${DNS_ZONE}","records":[{"record_id":"record-www-a","name":"www","record_type":"a","ttl_secs":30,"values":[],"targets":[{"target_id":"target-node-agent","kind":"node","value":"${NODE_ID}","weight":100,"enabled":true,"source_cidrs":[],"tags":["smoke"]}]}] }]}}
JSON
)")"
DRAFT_ID="$(printf '%s' "${CREATE_DRAFT_RESPONSE}" | json_field "draft_id")"
[[ -n "${DRAFT_ID}" ]] || die "dns draft_id was missing"

log "validating dns draft ${DRAFT_ID}"
VALIDATE_DRAFT_RESPONSE="$(api_post_empty "/api/v1/dns/revisions/drafts/${DRAFT_ID}/validate")"
VALIDATION_OK="$(printf '%s' "${VALIDATE_DRAFT_RESPONSE}" | json_field "last_validation.valid")"
[[ "${VALIDATION_OK}" == "true" ]] || die "dns draft validation did not succeed: ${VALIDATE_DRAFT_RESPONSE}"

log "publishing dns draft ${DRAFT_ID}"
PUBLISH_DRAFT_RESPONSE="$(api_post_json \
    "/api/v1/dns/revisions/drafts/${DRAFT_ID}/publish" \
    '{"version_label":"dns-smoke-v1","summary":"control-plane dns smoke revision"}')"
REVISION_ID="$(printf '%s' "${PUBLISH_DRAFT_RESPONSE}" | json_field "revision.revision_id")"
[[ -n "${REVISION_ID}" ]] || die "published dns revision_id was missing"

log "creating dns deployment for revision ${REVISION_ID}"
CREATE_DEPLOYMENT_RESPONSE="$(api_post_json \
    "/api/v1/dns/deployments" \
    "$(cat <<JSON
{"cluster_id":"${CLUSTER_ID}","revision_id":"${REVISION_ID}","target_node_ids":["${NODE_ID}"],"parallelism":1,"failure_threshold":1,"auto_rollback":false}
JSON
)")"
DNS_DEPLOYMENT_ID="$(printf '%s' "${CREATE_DEPLOYMENT_RESPONSE}" | json_field "deployment.deployment.deployment_id")"
[[ -n "${DNS_DEPLOYMENT_ID}" ]] || die "dns deployment_id was missing"

log "checking dashboard visibility for dns deployment ${DNS_DEPLOYMENT_ID}"
DASHBOARD_RESPONSE="$(wait_for_dashboard_dns_deployment_active "${DNS_DEPLOYMENT_ID}" "${REVISION_ID}")"
JSON_PAYLOAD="${DASHBOARD_RESPONSE}" python3 - "${DNS_DEPLOYMENT_ID}" "${REVISION_ID}" <<'PY'
import json
import os
import sys

deployment_id = sys.argv[1]
revision_id = sys.argv[2]
payload = json.loads(os.environ["JSON_PAYLOAD"])
if int(payload["active_dns_deployments"]) < 1:
    raise SystemExit(1)
items = payload.get("recent_dns_deployments") or []
match = next((item for item in items if item.get("deployment_id") == deployment_id), None)
if match is None:
    raise SystemExit(1)
if match.get("revision_id") != revision_id:
    raise SystemExit(1)
PY

log "checking dns deployment event stream for ${DNS_DEPLOYMENT_ID}"
DNS_DEPLOYMENT_EVENT="$(fetch_dns_deployment_sse_event "${DNS_DEPLOYMENT_ID}")"
JSON_PAYLOAD="${DNS_DEPLOYMENT_EVENT}" python3 - "${DNS_DEPLOYMENT_ID}" "${REVISION_ID}" <<'PY'
import json
import os
import sys

deployment_id = sys.argv[1]
revision_id = sys.argv[2]
payload = json.loads(os.environ["JSON_PAYLOAD"])
if payload.get("event") != "dns_deployment.tick":
    raise SystemExit(1)
data = payload.get("data") or {}
detail = data.get("detail") or {}
deployment = detail.get("deployment") or {}
if deployment.get("deployment_id") != deployment_id:
    raise SystemExit(1)
if deployment.get("revision_id") != revision_id:
    raise SystemExit(1)
PY

log "waiting for dns deployment ${DNS_DEPLOYMENT_ID} to succeed"
wait_for_dns_deployment_status "${DNS_DEPLOYMENT_ID}" "succeeded" >/dev/null

log "checking metrics exposure for dns deployment ${DNS_DEPLOYMENT_ID}"
DNS_DEPLOYMENT_METRICS="$(wait_for_dns_deployment_metrics "${DNS_DEPLOYMENT_ID}" "${REVISION_ID}")"
METRICS_PAYLOAD="${DNS_DEPLOYMENT_METRICS}" python3 - "${DNS_DEPLOYMENT_ID}" <<'PY'
import os
import sys

deployment_id = sys.argv[1]
payload = os.environ["METRICS_PAYLOAD"]
if f'deployment_id="{deployment_id}"' not in payload:
    raise SystemExit(1)
if "rginx_control_dns_deployment_info" not in payload:
    raise SystemExit(1)
if "rginx_control_dns_deployment_targets" not in payload:
    raise SystemExit(1)
PY

log "checking control-plane dns runtime state for revision ${REVISION_ID}"
RUNTIME_RESPONSE="$(api_get "/api/v1/dns/runtime")"
JSON_PAYLOAD="${RUNTIME_RESPONSE}" python3 - "${CLUSTER_ID}" "${REVISION_ID}" <<'PY'
import json
import os
import sys

cluster_id = sys.argv[1]
revision_id = sys.argv[2]
rows = json.loads(os.environ["JSON_PAYLOAD"])

for row in rows:
    if row.get("cluster_id") == cluster_id and row.get("published_revision_id") == revision_id:
        raise SystemExit(0)

raise SystemExit(1)
PY

log "running dns simulation for ${DNS_QNAME}"
SIMULATE_RESPONSE="$(api_post_json \
    "/api/v1/dns/simulate" \
    "{\"cluster_id\":\"${CLUSTER_ID}\",\"qname\":\"${DNS_QNAME}\",\"record_type\":\"a\",\"source_ip\":\"198.51.100.10\",\"revision_id\":\"${REVISION_ID}\",\"draft_id\":null}")"
JSON_PAYLOAD="${SIMULATE_RESPONSE}" python3 - "${DNS_EXPECTED_IP}" <<'PY'
import json
import os
import sys

expected_ip = sys.argv[1]
payload = json.loads(os.environ["JSON_PAYLOAD"])
answers = payload.get("answers") or []
if not answers:
    raise SystemExit(1)
first = answers[0]
if first.get("value") != expected_ip:
    raise SystemExit(1)
if first.get("target_kind") != "node":
    raise SystemExit(1)
PY

log "waiting for node snapshot to report dns revision ${REVISION_ID}"
wait_for_node_dns_revision "${REVISION_ID}" >/dev/null

log "querying local authoritative dns over udp"
run_dns_query "udp" "${NODE_DNS_UDP_ADDR}" "${DNS_QNAME}" "${DNS_EXPECTED_IP}"

log "querying local authoritative dns over tcp"
run_dns_query "tcp" "${NODE_DNS_TCP_ADDR}" "${DNS_QNAME}" "${DNS_EXPECTED_IP}"

log "querying missing authoritative dns over udp"
run_dns_nxdomain_query "udp" "${NODE_DNS_UDP_ADDR}" "missing.${DNS_ZONE}"

log "waiting for node snapshot to observe dns query counters"
NODE_DETAIL_RESPONSE="$(wait_for_node_query_total 3)"
JSON_PAYLOAD="${NODE_DETAIL_RESPONSE}" python3 - "${DNS_EXPECTED_IP}" "${REVISION_ID}" "${DNS_QNAME}" "missing.${DNS_ZONE}" <<'PY'
import json
import os
import sys

expected_ip = sys.argv[1]
revision_id = sys.argv[2]
hot_qname = sys.argv[3]
error_qname = sys.argv[4]
payload = json.loads(os.environ["JSON_PAYLOAD"])
snapshot = payload["latest_snapshot"]
status = snapshot["status"]
dns = status["dns"]

if dns["published_revision_id"] != revision_id:
    raise SystemExit(1)
if int(dns["query_total"]) < 3:
    raise SystemExit(1)
if dns["response_noerror_total"] < 2:
    raise SystemExit(1)
if dns["response_nxdomain_total"] < 1:
    raise SystemExit(1)
if payload["node"]["advertise_addr"].split(":")[0] != expected_ip:
    raise SystemExit(1)

hot_queries = dns.get("hot_queries") or []
hot = next((item for item in hot_queries if item.get("qname") == hot_qname), None)
if hot is None:
    raise SystemExit(1)
if int(hot.get("query_total", 0)) < 2:
    raise SystemExit(1)
if int(hot.get("answer_total", 0)) < 2:
    raise SystemExit(1)

error_queries = dns.get("error_queries") or []
error = next((item for item in error_queries if item.get("qname") == error_qname), None)
if error is None:
    raise SystemExit(1)
if int(error.get("response_nxdomain_total", 0)) < 1:
    raise SystemExit(1)
PY

log "control-plane dns end-to-end smoke test passed"
