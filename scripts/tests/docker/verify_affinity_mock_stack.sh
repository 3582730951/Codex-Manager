#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
COMPOSE_FILE="${ROOT_DIR}/scripts/tests/docker/affinity_mock_stack.compose.yml"
STAMP="$(date +%Y%m%d%H%M%S)-$$-${RANDOM}"
TEST_PROJECT="codexmanager-affinity-${STAMP}"
SERVICE_PORT=""
WEB_PORT=""
KEEP_TEST_STACK="0"
SKIP_DESKTOP_BUILD="0"
PLATFORM_KEY="cm_affinity_test_key"
DATA_VOLUME=""
ENV_FILE=""
NETWORK_NAME=""
PROBE_CONTAINER=""
PROBE_A_CONTAINER=""
PROBE_B_CONTAINER=""
NETWORK_SUBNET="172.29.0.0/24"
SERVICE_IP="172.29.0.10"
WEB_IP="172.29.0.11"
PROBE_A_IP="172.29.0.21"
PROBE_B_IP="172.29.0.22"
MOCK_UPSTREAM_IP="172.29.0.30"
TRUSTED_PEERS="${PROBE_A_IP}/32,${PROBE_B_IP}/32"
DESKTOP_BUILDER_IMAGE=""
TEST_PYTHON_IMAGE=""

log() { printf '\n[%s] %s\n' "$(date '+%H:%M:%S')" "$*"; }
die() { printf '\n[ERROR] %s\n' "$*" >&2; exit 1; }
need_cmd() { command -v "$1" >/dev/null 2>&1 || die "missing command: $1"; }

allocate_host_port() {
  python3 - <<'PY'
import socket

sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --test-project)
      TEST_PROJECT="$2"
      shift 2
      ;;
    --service-port)
      SERVICE_PORT="$2"
      shift 2
      ;;
    --web-port)
      WEB_PORT="$2"
      shift 2
      ;;
    --keep-test-stack)
      KEEP_TEST_STACK="1"
      shift
      ;;
    --skip-desktop-build)
      SKIP_DESKTOP_BUILD="1"
      shift
      ;;
    *)
      die "unknown arg: $1"
      ;;
  esac
done

need_cmd docker
need_cmd python3

for name in CODEX_API_KEY OPENAI_API_KEY OPENAI_API_BASE CODEX_API_BASE; do
  if [[ -n "${!name:-}" ]]; then
    die "refusing to run with ${name} set; mock affinity verification must not use real upstream credentials"
  fi
done

if [[ -z "${SERVICE_PORT}" ]]; then
  SERVICE_PORT="$(allocate_host_port)"
fi
if [[ -z "${WEB_PORT}" ]]; then
  while true; do
    WEB_PORT="$(allocate_host_port)"
    [[ "${WEB_PORT}" != "${SERVICE_PORT}" ]] && break
  done
fi

run_desktop_build() {
  DESKTOP_BUILDER_IMAGE="${TEST_PROJECT}-desktop-builder"
  docker build \
    -f "${ROOT_DIR}/docker/Dockerfile.desktop-test" \
    -t "${DESKTOP_BUILDER_IMAGE}" \
    "${ROOT_DIR}" >/dev/null
  docker run --rm \
    -v "${ROOT_DIR}/apps:/src:ro" \
    "${DESKTOP_BUILDER_IMAGE}" \
    bash -lc "set -euo pipefail && corepack enable >/dev/null 2>&1 && corepack prepare pnpm@10.30.3 --activate >/dev/null 2>&1 && mkdir -p /tmp/apps && cp -a /src/. /tmp/apps/ && rm -rf /tmp/apps/node_modules /tmp/apps/.next /tmp/apps/out && cd /tmp/apps && export NEXT_TELEMETRY_DISABLED=1 CI=true && pnpm install --frozen-lockfile && pnpm run build:desktop"
}

ensure_test_python_image() {
  if [[ -n "${TEST_PYTHON_IMAGE}" ]]; then
    return 0
  fi
  TEST_PYTHON_IMAGE="${TEST_PROJECT}-test-python"
  docker build \
    -f "${ROOT_DIR}/docker/Dockerfile.test-python" \
    -t "${TEST_PYTHON_IMAGE}" \
    "${ROOT_DIR}" >/dev/null
}

set_env_file_value() {
  local key="$1"
  local value="$2"
  python3 - "${ENV_FILE}" "${key}" "${value}" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
key = sys.argv[2]
value = sys.argv[3]
lines = []
found = False
for line in path.read_text(encoding="utf-8").splitlines():
    if line.startswith(f"{key}="):
        lines.append(f"{key}={value}")
        found = True
    else:
        lines.append(line)
if not found:
    lines.append(f"{key}={value}")
path.write_text("\n".join(lines) + "\n", encoding="utf-8")
PY
}

recreate_service_stack() {
  docker compose -p "${TEST_PROJECT}" --env-file "${ENV_FILE}" -f "${COMPOSE_FILE}" up -d --build --force-recreate service-test web-test >/dev/null
  wait_http_ok "http://service-test:48760/health"
  wait_http_ok "http://web-test:48761/api/runtime"
  sleep 6
}

set_client_entity_mode() {
  local mode="$1"
  set_env_file_value "AFFINITY_TEST_CLIENT_ENTITY_MODE" "${mode}"
  recreate_service_stack
}

set_trusted_peers() {
  local peers="$1"
  set_env_file_value "AFFINITY_TEST_TRUSTED_PEERS" "${peers}"
}

wait_http_ok() {
  local url="$1"
  for _ in $(seq 1 90); do
    if docker exec -i "${PROBE_CONTAINER}" python - "${url}" <<'PY' >/dev/null 2>&1
import sys
import urllib.error
import urllib.request

url = sys.argv[1]
with urllib.request.urlopen(url, timeout=5) as response:
    if response.status != 200:
        raise SystemExit(1)
PY
    then
      return 0
    fi
    sleep 2
  done
  die "endpoint not ready: ${url}"
}

wait_seed_success() {
  local seed_id
  seed_id="$(docker compose -p "${TEST_PROJECT}" --env-file "${ENV_FILE}" -f "${COMPOSE_FILE}" ps -a -q seed-init)"
  [[ -n "${seed_id}" ]] || die "seed-init container missing"
  for _ in $(seq 1 90); do
    local status
    status="$(docker inspect --format '{{.State.Status}} {{.State.ExitCode}}' "${seed_id}")"
    if [[ "${status}" == "exited 0" ]]; then
      return 0
    fi
    if [[ "${status}" == exited* ]]; then
      docker logs "${seed_id}" >&2 || true
      die "seed-init failed with status: ${status}"
    fi
    sleep 2
  done
  docker logs "${seed_id}" >&2 || true
  die "seed-init timed out"
}

db_python() {
  ensure_test_python_image
  docker run --rm -i -v "${DATA_VOLUME}:/data" "${TEST_PYTHON_IMAGE}" python -
}

clear_affinity_state() {
  db_python <<'PY'
import sqlite3
conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
for table in [
    "client_bindings",
    "conversation_threads",
    "conversation_context_state",
    "conversation_context_events",
    "context_snapshots",
    "conversation_bindings",
    "account_quota_exhaustion",
]:
    cur.execute(f"DELETE FROM {table}")
conn.commit()
conn.close()
PY
}

apply_usage_map() {
  local usage_json="$1"
  ensure_test_python_image
  docker run --rm -i -e USAGE_JSON="${usage_json}" -v "${DATA_VOLUME}:/data" "${TEST_PYTHON_IMAGE}" python - <<'PY'
import json
import os
import sqlite3
import time

usage = json.loads(os.environ["USAGE_JSON"])
now = int(time.time())
conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
for account_id, used_percent in usage.items():
    cur.execute(
        """
        INSERT INTO usage_snapshots (
          account_id, used_percent, window_minutes, resets_at, secondary_used_percent,
          secondary_window_minutes, secondary_resets_at, credits_json, captured_at
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        """,
        (account_id, float(used_percent), 60, now + 3600, None, None, None, None, now),
    )
conn.commit()
conn.close()
PY
}

restore_mock_tokens() {
  db_python <<'PY'
import sqlite3

updates = {
    "aff-acc-1": "mock-account-1",
    "aff-acc-2": "mock-account-2",
    "aff-acc-3": "mock-account-3",
    "aff-acc-4": "mock-account-4",
    "aff-acc-5": "mock-account-5",
}
conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
for account_id, access_token in updates.items():
    cur.execute(
        "UPDATE tokens SET access_token = ?, id_token = ?, refresh_token = ? WHERE account_id = ?",
        (access_token, f"id-{access_token}", f"refresh-{access_token}", account_id),
    )
    cur.execute(
        "UPDATE accounts SET status = 'active', updated_at = strftime('%s','now') WHERE id = ?",
        (account_id,),
    )
conn.commit()
conn.close()
PY
}

set_mock_token() {
  local account_id="$1"
  local token_value="$2"
  ensure_test_python_image
  docker run --rm -i \
    -e ACCOUNT_ID="${account_id}" \
    -e TOKEN_VALUE="${token_value}" \
    -v "${DATA_VOLUME}:/data" \
    "${TEST_PYTHON_IMAGE}" python - <<'PY'
import os
import sqlite3

account_id = os.environ["ACCOUNT_ID"]
token_value = os.environ["TOKEN_VALUE"]
conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
cur.execute(
    "UPDATE tokens SET access_token = ?, id_token = ?, refresh_token = ? WHERE account_id = ?",
    (token_value, f"id-{token_value}", f"refresh-{token_value}", account_id),
)
conn.commit()
conn.close()
PY
}

binding_counts() {
  db_python <<'PY'
import sqlite3
conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
rows = cur.execute(
    "SELECT account_id, COUNT(1) FROM client_bindings GROUP BY account_id ORDER BY account_id"
).fetchall()
for account_id, count in rows:
    print(f"{account_id}={count}")
conn.close()
PY
}

assert_binding_counts() {
  local expected="$1"
  local actual
  actual="$(binding_counts)"
  if [[ "${actual}" != "${expected}" ]]; then
    printf '\nExpected bindings:\n%s\n\nActual bindings:\n%s\n' "${expected}" "${actual}" >&2
    die "binding counts mismatch"
  fi
}

assert_no_bindings() { assert_binding_counts ""; }

assert_no_affinity_persistence() {
  local actual
  actual="$(db_python <<'PY'
import sqlite3

conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
tables = [
    "client_bindings",
    "conversation_bindings",
    "conversation_threads",
    "conversation_context_state",
    "conversation_context_events",
    "context_snapshots",
    "account_quota_exhaustion",
]
for table in tables:
    count = cur.execute(f"SELECT COUNT(1) FROM {table}").fetchone()[0]
    print(f"{table}={count}")
conn.close()
PY
)"
  local expected=$'client_bindings=0\nconversation_bindings=0\nconversation_threads=0\nconversation_context_state=0\nconversation_context_events=0\ncontext_snapshots=0\naccount_quota_exhaustion=0'
  if [[ "${actual}" != "${expected}" ]]; then
    printf '\nExpected zero persistence:\n%s\n\nActual persistence:\n%s\n' "${expected}" "${actual}" >&2
    die "affinity persistence expected to remain empty"
  fi
}

assert_affinity_bound_to() {
  local affinity_prefix="$1"
  local affinity_id="$2"
  local expected_account="$3"
  local actual
  ensure_test_python_image
  actual="$(docker run --rm -i -e AFFINITY_PREFIX="${affinity_prefix}" -e AFFINITY_ID="${affinity_id}" -v "${DATA_VOLUME}:/data" "${TEST_PYTHON_IMAGE}" python - <<'PY'
import os
import sqlite3

affinity_prefix = os.environ["AFFINITY_PREFIX"]
affinity_id = os.environ["AFFINITY_ID"]
conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
row = cur.execute(
    "SELECT account_id FROM client_bindings WHERE affinity_key = ? LIMIT 1",
    (f"{affinity_prefix}:{affinity_id}",),
).fetchone()
print("" if row is None else row[0])
conn.close()
PY
)"
  [[ "${actual}" == "${expected_account}" ]] || die "affinity ${affinity_prefix}:${affinity_id} expected ${expected_account}, got ${actual}"
}

assert_affinity_unbound() {
  local affinity_prefix="$1"
  local affinity_id="$2"
  local actual
  ensure_test_python_image
  actual="$(docker run --rm -i -e AFFINITY_PREFIX="${affinity_prefix}" -e AFFINITY_ID="${affinity_id}" -v "${DATA_VOLUME}:/data" "${TEST_PYTHON_IMAGE}" python - <<'PY'
import os
import sqlite3

affinity_prefix = os.environ["AFFINITY_PREFIX"]
affinity_id = os.environ["AFFINITY_ID"]
conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
row = cur.execute(
    "SELECT account_id FROM client_bindings WHERE affinity_key = ? LIMIT 1",
    (f"{affinity_prefix}:{affinity_id}",),
).fetchone()
print("" if row is None else row[0])
conn.close()
PY
)"
  [[ -z "${actual}" ]] || die "affinity ${affinity_prefix}:${affinity_id} expected no binding, got ${actual}"
}

assert_context_events_at_least() {
  local affinity_prefix="$1"
  local affinity_id="$2"
  local min_count="$3"
  local actual
  ensure_test_python_image
  actual="$(docker run --rm -i -e AFFINITY_PREFIX="${affinity_prefix}" -e AFFINITY_ID="${affinity_id}" -v "${DATA_VOLUME}:/data" "${TEST_PYTHON_IMAGE}" python - <<'PY'
import os
import sqlite3

affinity_prefix = os.environ["AFFINITY_PREFIX"]
affinity_id = os.environ["AFFINITY_ID"]
conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
row = cur.execute(
    "SELECT COUNT(1) FROM conversation_context_events WHERE affinity_key = ?",
    (f"{affinity_prefix}:{affinity_id}",),
).fetchone()
print(row[0] if row else 0)
conn.close()
PY
)"
  [[ "${actual}" -ge "${min_count}" ]] || die "affinity ${affinity_prefix}:${affinity_id} expected at least ${min_count} context events, got ${actual}"
}

assert_context_events_exact() {
  local affinity_prefix="$1"
  local affinity_id="$2"
  local expected_count="$3"
  local actual
  ensure_test_python_image
  actual="$(docker run --rm -i -e AFFINITY_PREFIX="${affinity_prefix}" -e AFFINITY_ID="${affinity_id}" -v "${DATA_VOLUME}:/data" "${TEST_PYTHON_IMAGE}" python - <<'PY'
import os
import sqlite3

affinity_prefix = os.environ["AFFINITY_PREFIX"]
affinity_id = os.environ["AFFINITY_ID"]
conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
row = cur.execute(
    "SELECT COUNT(1) FROM conversation_context_events WHERE affinity_key = ?",
    (f"{affinity_prefix}:{affinity_id}",),
).fetchone()
print(row[0] if row else 0)
conn.close()
PY
)"
  [[ "${actual}" == "${expected_count}" ]] || die "affinity ${affinity_prefix}:${affinity_id} expected exactly ${expected_count} context events, got ${actual}"
}

latest_child_key_details() {
  local owner_key_id="$1"
  ensure_test_python_image
  docker run --rm -i -e OWNER_KEY_ID="${owner_key_id}" -v "${DATA_VOLUME}:/data" "${TEST_PYTHON_IMAGE}" python - <<'PY'
import os
import sqlite3

owner_key_id = os.environ["OWNER_KEY_ID"]
conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
row = cur.execute(
    """
    SELECT child_key_id, cli_instance_uuid
    FROM cli_child_keys
    WHERE owner_key_id = ?
    ORDER BY created_at DESC, child_key_id DESC
    LIMIT 1
    """,
    (owner_key_id,),
).fetchone()
print("" if row is None else f"{row[0]}|{row[1]}")
conn.close()
PY
}

assert_latest_response_log_identity() {
  local expected_owner_key_id="$1"
  local expected_key_id="$2"
  local actual
  ensure_test_python_image
  actual="$(docker run --rm -i -v "${DATA_VOLUME}:/data" "${TEST_PYTHON_IMAGE}" python - <<'PY'
import sqlite3

conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
row = cur.execute(
    """
    SELECT COALESCE(owner_key_id, ''), COALESCE(key_id, '')
    FROM request_logs
    WHERE request_path = '/v1/responses'
    ORDER BY created_at DESC, id DESC
    LIMIT 1
    """
).fetchone()
print("" if row is None else f"{row[0]}|{row[1]}")
conn.close()
PY
)"
  local expected="${expected_owner_key_id}|${expected_key_id}"
  [[ "${actual}" == "${expected}" ]] || die "latest /v1/responses log expected ${expected}, got ${actual}"
}

assert_account_quota_exhausted() {
  local account_id="$1"
  local actual
  ensure_test_python_image
  actual="$(docker run --rm -i -e ACCOUNT_ID="${account_id}" -v "${DATA_VOLUME}:/data" "${TEST_PYTHON_IMAGE}" python - <<'PY'
import os
import sqlite3

account_id = os.environ["ACCOUNT_ID"]
conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
row = cur.execute(
    "SELECT exhausted_until FROM account_quota_exhaustion WHERE account_id = ? LIMIT 1",
    (account_id,),
).fetchone()
print("" if row is None else row[0])
conn.close()
PY
)"
  [[ -n "${actual}" ]] || die "expected hard quota exhaustion for ${account_id}"
}

restart_service() {
  docker compose -p "${TEST_PROJECT}" --env-file "${ENV_FILE}" -f "${COMPOSE_FILE}" restart service-test >/dev/null
  wait_http_ok "http://service-test:48760/health"
  sleep 6
}

send_turn_raw_via() {
  local probe_container="$1"
  local base_url="$2"
  local affinity_id="$3"
  local text="$4"
  local affinity_mode="${5:-cli}"
  docker exec -i "${probe_container}" python - "${PLATFORM_KEY}" "${base_url}" "${affinity_id}" "${text}" "${affinity_mode}" <<'PY'
import json
import sys
import urllib.error
import urllib.request

platform_key = sys.argv[1]
base_url = sys.argv[2]
affinity_id = sys.argv[3]
text = sys.argv[4]
affinity_mode = sys.argv[5]
headers = {
    "Authorization": f"Bearer {platform_key}",
    "Content-Type": "application/json",
}
if affinity_mode == "cli":
    headers["x-codex-cli-affinity-id"] = affinity_id
elif affinity_mode == "session":
    headers["session_id"] = affinity_id
elif affinity_mode == "conversation":
    headers["conversation_id"] = affinity_id
elif affinity_mode == "none":
    pass
else:
    raise SystemExit(f"unsupported affinity mode: {affinity_mode}")
request = urllib.request.Request(
    f"{base_url}/v1/responses",
    data=json.dumps(
        {
            "model": "gpt-5.4",
            "stream": False,
            "input": [
                {
                    "role": "user",
                    "content": [{"type": "input_text", "text": text}],
                }
            ],
        }
    ).encode("utf-8"),
    headers=headers,
    method="POST",
)
try:
    with urllib.request.urlopen(request, timeout=30) as response:
        body = response.read().decode("utf-8")
        assistant_text = body
        try:
            parsed = json.loads(body)
            for item in parsed.get("output") or []:
                if not isinstance(item, dict):
                    continue
                for content in item.get("content") or []:
                    if not isinstance(content, dict):
                        continue
                    text_value = content.get("text")
                    if isinstance(text_value, str) and text_value:
                        assistant_text = text_value
                        raise StopIteration
        except StopIteration:
            pass
        except Exception:
            assistant_text = body
        print(assistant_text)
        print(response.status)
except urllib.error.HTTPError as error:
    body = error.read().decode("utf-8")
    print(body)
    print(error.code)
PY
}

send_turn_raw_auth_via() {
  local probe_container="$1"
  local base_url="$2"
  local auth_token="$3"
  local affinity_id="$4"
  local text="$5"
  local affinity_mode="${6:-cli}"
  docker exec -i "${probe_container}" python - "${auth_token}" "${base_url}" "${affinity_id}" "${text}" "${affinity_mode}" <<'PY'
import json
import sys
import urllib.error
import urllib.request

auth_token = sys.argv[1]
base_url = sys.argv[2]
affinity_id = sys.argv[3]
text = sys.argv[4]
affinity_mode = sys.argv[5]
headers = {
    "Authorization": f"Bearer {auth_token}",
    "Content-Type": "application/json",
}
if affinity_mode == "cli":
    headers["x-codex-cli-affinity-id"] = affinity_id
elif affinity_mode == "session":
    headers["session_id"] = affinity_id
elif affinity_mode == "conversation":
    headers["conversation_id"] = affinity_id
elif affinity_mode == "none":
    pass
else:
    raise SystemExit(f"unsupported affinity mode: {affinity_mode}")
request = urllib.request.Request(
    f"{base_url}/v1/responses",
    data=json.dumps(
        {
            "model": "gpt-5.4",
            "stream": False,
            "input": [
                {
                    "role": "user",
                    "content": [{"type": "input_text", "text": text}],
                }
            ],
        }
    ).encode("utf-8"),
    headers=headers,
    method="POST",
)
try:
    with urllib.request.urlopen(request, timeout=30) as response:
        body = response.read().decode("utf-8")
        assistant_text = body
        try:
            parsed = json.loads(body)
            for item in parsed.get("output") or []:
                if not isinstance(item, dict):
                    continue
                for content in item.get("content") or []:
                    if not isinstance(content, dict):
                        continue
                    text_value = content.get("text")
                    if isinstance(text_value, str) and text_value:
                        assistant_text = text_value
                        raise StopIteration
        except StopIteration:
            pass
        except Exception:
            assistant_text = body
        print(assistant_text)
        print(response.status)
except urllib.error.HTTPError as error:
    body = error.read().decode("utf-8")
    print(body)
    print(error.code)
PY
}

send_turn_stream_raw_auth_via() {
  local probe_container="$1"
  local base_url="$2"
  local auth_token="$3"
  local affinity_id="$4"
  local text="$5"
  local affinity_mode="${6:-cli}"
  docker exec -i "${probe_container}" python - "${auth_token}" "${base_url}" "${affinity_id}" "${text}" "${affinity_mode}" <<'PY'
import json
import sys
import urllib.error
import urllib.request

auth_token = sys.argv[1]
base_url = sys.argv[2]
affinity_id = sys.argv[3]
text = sys.argv[4]
affinity_mode = sys.argv[5]
headers = {
    "Authorization": f"Bearer {auth_token}",
    "Content-Type": "application/json",
}
if affinity_mode == "cli":
    headers["x-codex-cli-affinity-id"] = affinity_id
elif affinity_mode == "session":
    headers["session_id"] = affinity_id
elif affinity_mode == "conversation":
    headers["conversation_id"] = affinity_id
elif affinity_mode == "none":
    pass
else:
    raise SystemExit(f"unsupported affinity mode: {affinity_mode}")
request = urllib.request.Request(
    f"{base_url}/v1/responses",
    data=json.dumps(
        {
            "model": "gpt-5.4",
            "stream": True,
            "input": [
                {
                    "role": "user",
                    "content": [{"type": "input_text", "text": text}],
                }
            ],
        }
    ).encode("utf-8"),
    headers=headers,
    method="POST",
)
try:
    with urllib.request.urlopen(request, timeout=30) as response:
        body = response.read().decode("utf-8")
        print(body)
        print(response.status)
except urllib.error.HTTPError as error:
    body = error.read().decode("utf-8")
    print(body)
    print(error.code)
PY
}

mint_cli_child_key_via_oauth() {
  local probe_container="$1"
  local base_url="$2"
  local employee_key="$3"
  local client_id="${4:-cli-child-test}"
  local redirect_uri="${5:-http://127.0.0.1:1455/callback}"
  local verifier="${6:-cli-child-verifier}"
  docker exec -i "${probe_container}" python - "${base_url}" "${employee_key}" "${client_id}" "${redirect_uri}" "${verifier}" <<'PY'
import base64
import hashlib
import json
import sys
import urllib.parse
import urllib.request

base_url, employee_key, client_id, redirect_uri, verifier = sys.argv[1:6]
challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode("utf-8")).digest()).decode("ascii").rstrip("=")

class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None

    http_error_301 = http_error_302 = http_error_303 = http_error_307 = http_error_308 = (
        lambda self, req, fp, code, msg, headers: fp
    )

opener = urllib.request.build_opener(NoRedirect)
auth_body = urllib.parse.urlencode(
    {
        "response_type": "code",
        "client_id": client_id,
        "redirect_uri": redirect_uri,
        "state": "cli-child-state",
        "code_challenge": challenge,
        "code_challenge_method": "S256",
        "employee_api_key": employee_key,
    }
).encode("utf-8")
auth_request = urllib.request.Request(
    f"{base_url}/oauth/authorize/approve",
    data=auth_body,
    headers={"Content-Type": "application/x-www-form-urlencoded"},
    method="POST",
)
auth_response = opener.open(auth_request, timeout=30)
location = auth_response.headers.get("Location")
if not location:
    raise SystemExit("missing authorize redirect")
query = urllib.parse.parse_qs(urllib.parse.urlparse(location).query)
code = query.get("code", [None])[0]
if not code:
    raise SystemExit("missing authorize code")

token_body = urllib.parse.urlencode(
    {
        "grant_type": "authorization_code",
        "code": code,
        "redirect_uri": redirect_uri,
        "client_id": client_id,
        "code_verifier": verifier,
    }
).encode("utf-8")
token_request = urllib.request.Request(
    f"{base_url}/oauth/token",
    data=token_body,
    headers={"Content-Type": "application/x-www-form-urlencoded"},
    method="POST",
)
with urllib.request.urlopen(token_request, timeout=30) as token_response:
    token_payload = json.loads(token_response.read().decode("utf-8"))
id_token = token_payload.get("id_token")
if not id_token:
    raise SystemExit("missing id_token")

exchange_body = urllib.parse.urlencode(
    {
        "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
        "client_id": client_id,
        "requested_token": "openai-api-key",
        "subject_token_type": "urn:ietf:params:oauth:token-type:id_token",
        "subject_token": id_token,
    }
).encode("utf-8")
exchange_request = urllib.request.Request(
    f"{base_url}/oauth/token",
    data=exchange_body,
    headers={"Content-Type": "application/x-www-form-urlencoded"},
    method="POST",
)
with urllib.request.urlopen(exchange_request, timeout=30) as exchange_response:
    exchange_payload = json.loads(exchange_response.read().decode("utf-8"))
access_token = exchange_payload.get("access_token")
if not access_token:
    raise SystemExit("missing child access token")
print(access_token)
PY
}

send_turn_raw() {
  local affinity_id="$1"
  local text="$2"
  local affinity_mode="${3:-cli}"
  send_turn_raw_via "${PROBE_CONTAINER}" "http://service-test:48760" "${affinity_id}" "${text}" "${affinity_mode}"
}

send_turn() {
  local affinity_id="$1"
  local text="$2"
  local affinity_mode="${3:-cli}"
  local response
  response="$(send_turn_raw "${affinity_id}" "${text}" "${affinity_mode}")"
  local status="${response##*$'\n'}"
  [[ "${status}" == "200" ]] || die "request failed for ${affinity_mode}:${affinity_id} with status ${status}"
}

assert_response_status() {
  local response="$1"
  local expected_status="$2"
  local actual_status="${response##*$'\n'}"
  [[ "${actual_status}" == "${expected_status}" ]] || die "expected HTTP ${expected_status}, got ${actual_status}"
}

assert_response_account() {
  local response="$1"
  local expected_token="$2"
  local output_text="${response%$'\n'*}"
  [[ "${output_text}" == "mock:${expected_token}:"* ]] || die "expected response from ${expected_token}, got ${output_text}"
}

assert_response_contains() {
  local response="$1"
  local expected="$2"
  local output_text="${response%$'\n'*}"
  [[ "${output_text}" == *"${expected}"* ]] || die "response did not contain ${expected}: ${output_text}"
}

assert_response_not_contains() {
  local response="$1"
  local unexpected="$2"
  local output_text="${response%$'\n'*}"
  [[ "${output_text}" != *"${unexpected}"* ]] || die "response unexpectedly contained ${unexpected}: ${output_text}"
}

run_turn_batch() {
  local prefix="$1"
  local count="$2"
  local label="$3"
  local idx
  for idx in $(seq 1 "${count}"); do
    send_turn "${prefix}-${idx}" "${label}-${idx}"
  done
}

cleanup() {
  local exit_status=$?
  local cleanup_failed="0"
  if [[ "${KEEP_TEST_STACK}" == "1" ]]; then
    log "keeping mock stack ${TEST_PROJECT}"
    return
  fi
  [[ -n "${DESKTOP_BUILDER_IMAGE}" ]] && docker image rm "${DESKTOP_BUILDER_IMAGE}" >/dev/null 2>&1 || true
  [[ -n "${TEST_PYTHON_IMAGE}" ]] && docker image rm "${TEST_PYTHON_IMAGE}" >/dev/null 2>&1 || true
  docker compose -p "${TEST_PROJECT}" --env-file "${ENV_FILE}" -f "${COMPOSE_FILE}" down -v --remove-orphans >/dev/null 2>&1 || true
  [[ -n "${NETWORK_NAME}" ]] && docker network rm "${NETWORK_NAME}" >/dev/null 2>&1 || true
  [[ -n "${DATA_VOLUME}" ]] && docker volume rm "${DATA_VOLUME}" >/dev/null 2>&1 || true
  local containers
  containers="$(docker ps -a --format '{{.Names}}' | grep "^${TEST_PROJECT}-" || true)"
  local volumes
  volumes="$(docker volume ls --format '{{.Name}}' | grep "^${TEST_PROJECT}-" || true)"
  local networks
  networks="$(docker network ls --format '{{.Name}}' | grep "^${TEST_PROJECT}_" || true)"
  if [[ -n "${containers}" || -n "${volumes}" || -n "${networks}" ]]; then
    cleanup_failed="1"
    printf '\n[ERROR] cleanup residue detected for %s\n' "${TEST_PROJECT}" >&2
    [[ -n "${containers}" ]] && printf 'containers:\n%s\n' "${containers}" >&2
    [[ -n "${volumes}" ]] && printf 'volumes:\n%s\n' "${volumes}" >&2
    [[ -n "${networks}" ]] && printf 'networks:\n%s\n' "${networks}" >&2
  fi
  [[ -n "${ENV_FILE}" ]] && rm -f "${ENV_FILE}"
  if [[ "${exit_status}" -ne 0 ]]; then
    exit "${exit_status}"
  fi
  if [[ "${cleanup_failed}" != "0" ]]; then
    exit 1
  fi
}
trap cleanup EXIT

DATA_VOLUME="${TEST_PROJECT}-data"
NETWORK_NAME="${TEST_PROJECT}_affinity-net"
docker volume create "${DATA_VOLUME}" >/dev/null
ENV_FILE="$(mktemp "/tmp/codexmanager-affinity-env.XXXXXX")"
cat >"${ENV_FILE}" <<EOF
AFFINITY_TEST_DATA_VOLUME=${DATA_VOLUME}
AFFINITY_SERVICE_PORT=${SERVICE_PORT}
AFFINITY_WEB_PORT=${WEB_PORT}
AFFINITY_TEST_PLATFORM_KEY=${PLATFORM_KEY}
AFFINITY_TEST_CLIENT_ENTITY_MODE=off
AFFINITY_TEST_NETWORK_SUBNET=${NETWORK_SUBNET}
AFFINITY_TEST_TRUSTED_PEERS=
AFFINITY_TEST_SERVICE_IP=${SERVICE_IP}
AFFINITY_TEST_WEB_IP=${WEB_IP}
AFFINITY_TEST_PROBE_A_IP=${PROBE_A_IP}
AFFINITY_TEST_PROBE_B_IP=${PROBE_B_IP}
AFFINITY_TEST_MOCK_UPSTREAM_IP=${MOCK_UPSTREAM_IP}
EOF

cd "${ROOT_DIR}"

if [[ "${SKIP_DESKTOP_BUILD}" != "1" ]]; then
  log "running desktop build in container"
  run_desktop_build
fi

log "starting affinity mock stack ${TEST_PROJECT}"
docker compose -p "${TEST_PROJECT}" --env-file "${ENV_FILE}" -f "${COMPOSE_FILE}" up --build -d

PROBE_A_CONTAINER="${TEST_PROJECT}-probe-a-1"
PROBE_B_CONTAINER="${TEST_PROJECT}-probe-b-1"
PROBE_CONTAINER="${PROBE_A_CONTAINER}"

log "waiting for seed/init and endpoints"
wait_seed_success
wait_http_ok "http://service-test:48760/health"
wait_http_ok "http://web-test:48761/api/runtime"
sleep 6

log "scenario A: 5 CLI / 2 accounts initial split then soft-drain migration"
clear_affinity_state
restore_mock_tokens
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":40,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
run_turn_batch "case-a-cli" 5 "round-1"
assert_binding_counts $'aff-acc-1=3\naff-acc-2=2'

apply_usage_map '{"aff-acc-1":97,"aff-acc-2":40,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
sleep 6
run_turn_batch "case-a-cli" 5 "round-2"
assert_binding_counts 'aff-acc-2=5'
assert_affinity_bound_to "cli" "case-a-cli-1" "aff-acc-2"
assert_context_events_at_least "cli" "case-a-cli-1" 4

log "scenario B: 19 CLI / 3 accounts then middle account exhausted"
clear_affinity_state
restore_mock_tokens
apply_usage_map '{"aff-acc-1":30,"aff-acc-2":60,"aff-acc-3":80,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
run_turn_batch "case-b-cli" 19 "round-1"
assert_binding_counts $'aff-acc-1=10\naff-acc-2=6\naff-acc-3=3'

apply_usage_map '{"aff-acc-1":30,"aff-acc-2":100,"aff-acc-3":80,"aff-acc-4":100,"aff-acc-5":100}'
sleep 6
run_turn_batch "case-b-cli" 19 "round-2"
assert_binding_counts $'aff-acc-1=15\naff-acc-3=4'

log "scenario C: 3 CLI / 5 accounts should only use strongest accounts"
clear_affinity_state
restore_mock_tokens
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":20,"aff-acc-3":30,"aff-acc-4":80,"aff-acc-5":90}'
restart_service
run_turn_batch "case-c-cli" 3 "round-1"
assert_binding_counts $'aff-acc-1=1\naff-acc-2=1\naff-acc-3=1'

log "scenario D: hard quota on top candidate falls back before downstream sees quota"
clear_affinity_state
restore_mock_tokens
set_mock_token "aff-acc-1" "mock-account-1-quota-http-json"
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":40,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
response="$(send_turn_raw "case-d-cli-1" "quota-failover")"
assert_response_status "${response}" "200"
assert_response_account "${response}" "mock-account-2"
assert_response_not_contains "${response}" "usage limit"
assert_response_not_contains "${response}" "insufficient_quota"
assert_binding_counts 'aff-acc-2=1'
assert_affinity_bound_to "cli" "case-d-cli-1" "aff-acc-2"
assert_account_quota_exhausted "aff-acc-1"
set_mock_token "aff-acc-1" "mock-account-1"
response="$(send_turn_raw "case-d-cli-2" "quota-persisted-skip")"
assert_response_status "${response}" "200"
assert_response_account "${response}" "mock-account-2"
assert_affinity_bound_to "cli" "case-d-cli-2" "aff-acc-2"

log "scenario D2: OAuth child key hits /v1/responses with cli UUID affinity and SSE quota failover"
clear_affinity_state
restore_mock_tokens
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":40,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
child_token="$(mint_cli_child_key_via_oauth "${PROBE_CONTAINER}" "http://service-test:48760" "${PLATFORM_KEY}" "cli-child-test")"
child_meta="$(latest_child_key_details "aff-key-1")"
[[ -n "${child_meta}" ]] || die "expected cli child key metadata"
child_key_id="${child_meta%%|*}"
child_cli_uuid="${child_meta##*|}"
response="$(send_turn_raw_auth_via "${PROBE_CONTAINER}" "http://service-test:48760" "${child_token}" "child-key-no-header-1" "oauth-child-initial" "none")"
assert_response_status "${response}" "200"
assert_response_account "${response}" "mock-account-1"
assert_affinity_bound_to "cli" "${child_cli_uuid}" "aff-acc-1"
assert_latest_response_log_identity "aff-key-1" "${child_key_id}"
set_mock_token "aff-acc-1" "mock-account-1-quota-sse-failed"
response="$(send_turn_raw_auth_via "${PROBE_CONTAINER}" "http://service-test:48760" "${child_token}" "child-key-no-header-2" "oauth-child-failover" "none")"
assert_response_status "${response}" "200"
assert_response_account "${response}" "mock-account-2"
assert_response_not_contains "${response}" "usage limit"
assert_response_not_contains "${response}" "insufficient_quota"
assert_affinity_bound_to "cli" "${child_cli_uuid}" "aff-acc-2"
assert_context_events_at_least "cli" "${child_cli_uuid}" 4
assert_account_quota_exhausted "aff-acc-1"
assert_latest_response_log_identity "aff-key-1" "${child_key_id}"

log "scenario D3: SSE response.failed insufficient_quota falls back before downstream sees quota"
clear_affinity_state
restore_mock_tokens
set_mock_token "aff-acc-1" "mock-account-1-quota-sse-failed"
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":40,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
response="$(send_turn_raw "case-d3-cli-1" "quota-sse-failed-failover")"
assert_response_status "${response}" "200"
assert_response_account "${response}" "mock-account-2"
assert_response_not_contains "${response}" "usage limit"
assert_response_not_contains "${response}" "insufficient_quota"
assert_affinity_bound_to "cli" "case-d3-cli-1" "aff-acc-2"
assert_account_quota_exhausted "aff-acc-1"

log "scenario D4: completed SSE followed by extra usage_limit error still falls back"
clear_affinity_state
restore_mock_tokens
set_mock_token "aff-acc-1" "mock-account-1-quota-sse-extra"
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":40,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
response="$(send_turn_raw "case-d4-cli-1" "quota-sse-extra-failover")"
assert_response_status "${response}" "200"
assert_response_account "${response}" "mock-account-2"
assert_response_not_contains "${response}" "usage limit"
assert_response_not_contains "${response}" "insufficient_quota"
assert_affinity_bound_to "cli" "case-d4-cli-1" "aff-acc-2"
assert_account_quota_exhausted "aff-acc-1"

log "scenario D5: OAuth child key streaming SSE quota failover stays invisible downstream"
clear_affinity_state
restore_mock_tokens
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":40,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
child_token="$(mint_cli_child_key_via_oauth "${PROBE_CONTAINER}" "http://service-test:48760" "${PLATFORM_KEY}" "cli-child-stream-test")"
child_meta="$(latest_child_key_details "aff-key-1")"
[[ -n "${child_meta}" ]] || die "expected cli child key metadata for streaming case"
child_cli_uuid="${child_meta##*|}"
set_mock_token "aff-acc-1" "mock-account-1-quota-sse-extra"
response="$(send_turn_stream_raw_auth_via "${PROBE_CONTAINER}" "http://service-test:48760" "${child_token}" "child-key-stream-1" "oauth-child-stream-failover" "none")"
assert_response_status "${response}" "200"
assert_response_contains "${response}" "mock-account-2"
assert_response_not_contains "${response}" "usage limit"
assert_response_not_contains "${response}" "insufficient_quota"
assert_affinity_bound_to "cli" "${child_cli_uuid}" "aff-acc-2"
assert_account_quota_exhausted "aff-acc-1"

log "scenario E: challenge response on top candidate falls back to next account"
clear_affinity_state
restore_mock_tokens
set_mock_token "aff-acc-1" "mock-account-1-challenge"
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":40,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
send_turn "case-e-cli-1" "challenge-failover"
assert_binding_counts 'aff-acc-2=1'
assert_affinity_bound_to "cli" "case-e-cli-1" "aff-acc-2"

log "scenario F: upstream 5xx on top candidate falls back to next account"
clear_affinity_state
restore_mock_tokens
set_mock_token "aff-acc-1" "mock-account-1-5xx"
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":40,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
send_turn "case-f-cli-1" "server-failover"
assert_binding_counts 'aff-acc-2=1'
assert_affinity_bound_to "cli" "case-f-cli-1" "aff-acc-2"

log "scenario G: incomplete stream must not commit binding or context"
clear_affinity_state
restore_mock_tokens
set_mock_token "aff-acc-1" "mock-account-1-incomplete"
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":100,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
send_turn_raw "case-g-cli-1" "incomplete-stream" >/dev/null
assert_affinity_unbound "cli" "case-g-cli-1"
assert_context_events_exact "cli" "case-g-cli-1" 0

log "scenario H: no CLI header falls back to session affinity key"
clear_affinity_state
restore_mock_tokens
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":100,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
send_turn "case-h-session-1" "session-fallback" "session"
assert_affinity_bound_to "sid" "case-h-session-1" "aff-acc-1"

log "scenario I: no CLI/session header falls back to conversation affinity key"
clear_affinity_state
restore_mock_tokens
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":100,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
send_turn "case-i-conv-1" "conversation-fallback" "conversation"
assert_affinity_bound_to "cid" "case-i-conv-1" "aff-acc-1"

log "switching service-test to auto client-entity mode"
set_trusted_peers ""
set_client_entity_mode "auto"

log "scenario J: auto mode still honors explicit cli/session/conversation affinity"
clear_affinity_state
restore_mock_tokens
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":100,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
send_turn "case-j-cli-1" "auto-cli-explicit"
assert_affinity_bound_to "cli" "case-j-cli-1" "aff-acc-1"

clear_affinity_state
restore_mock_tokens
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":100,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
send_turn "case-j-session-1" "auto-session-explicit" "session"
assert_affinity_bound_to "sid" "case-j-session-1" "aff-acc-1"

clear_affinity_state
restore_mock_tokens
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":100,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
send_turn "case-j-conv-1" "auto-conversation-explicit" "conversation"
assert_affinity_bound_to "cid" "case-j-conv-1" "aff-acc-1"

log "scenario K: docker peer runtime keeps per-probe live pin without DB bindings"
clear_affinity_state
restore_mock_tokens
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":100,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
response="$(send_turn_raw_via "${PROBE_A_CONTAINER}" "http://service-test:48760" "case-k-none-a-1" "peer-runtime-probe-a-first" "none")"
assert_response_status "${response}" "200"
assert_response_account "${response}" "mock-account-1"
assert_no_affinity_persistence

apply_usage_map '{"aff-acc-1":100,"aff-acc-2":10,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
sleep 6
response="$(send_turn_raw_via "${PROBE_B_CONTAINER}" "http://service-test:48760" "case-k-none-b-1" "peer-runtime-probe-b-first" "none")"
assert_response_status "${response}" "200"
assert_response_account "${response}" "mock-account-2"
assert_no_affinity_persistence

apply_usage_map '{"aff-acc-1":10,"aff-acc-2":10,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
sleep 6
response="$(send_turn_raw_via "${PROBE_A_CONTAINER}" "http://service-test:48760" "case-k-none-a-2" "peer-runtime-probe-a-second" "none")"
assert_response_status "${response}" "200"
assert_response_account "${response}" "mock-account-1"
response="$(send_turn_raw_via "${PROBE_B_CONTAINER}" "http://service-test:48760" "case-k-none-b-2" "peer-runtime-probe-b-second" "none")"
assert_response_status "${response}" "200"
assert_response_account "${response}" "mock-account-2"
assert_no_affinity_persistence

log "scenario L: host-gateway path must not create trusted peer runtime pin"
clear_affinity_state
restore_mock_tokens
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":100,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
response="$(send_turn_raw_via "${PROBE_A_CONTAINER}" "http://host.docker.internal:${SERVICE_PORT}" "case-l-none-a-1" "host-gateway-negative" "none")"
assert_response_status "${response}" "200"
assert_response_account "${response}" "mock-account-1"
apply_usage_map '{"aff-acc-1":80,"aff-acc-2":10,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
sleep 6
response="$(send_turn_raw_via "${PROBE_A_CONTAINER}" "http://host.docker.internal:${SERVICE_PORT}" "case-l-none-a-2" "host-gateway-negative-second" "none")"
assert_response_status "${response}" "200"
assert_response_account "${response}" "mock-account-2"
assert_no_affinity_persistence

log "scenario M: explicit docker-peer-runtime subnet still excludes host-gateway"
set_trusted_peers "${NETWORK_SUBNET}"
set_client_entity_mode "docker-peer-runtime"
clear_affinity_state
restore_mock_tokens
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":100,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
response="$(send_turn_raw_via "${PROBE_A_CONTAINER}" "http://host.docker.internal:${SERVICE_PORT}" "case-m-none-a-1" "host-gateway-explicit-negative" "none")"
assert_response_status "${response}" "200"
assert_response_account "${response}" "mock-account-1"
apply_usage_map '{"aff-acc-1":80,"aff-acc-2":10,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
sleep 6
response="$(send_turn_raw_via "${PROBE_A_CONTAINER}" "http://host.docker.internal:${SERVICE_PORT}" "case-m-none-a-2" "host-gateway-explicit-negative-second" "none")"
assert_response_status "${response}" "200"
assert_response_account "${response}" "mock-account-2"
assert_no_affinity_persistence

docker compose -p "${TEST_PROJECT}" --env-file "${ENV_FILE}" -f "${COMPOSE_FILE}" ps
log "affinity mock stack verification complete"
