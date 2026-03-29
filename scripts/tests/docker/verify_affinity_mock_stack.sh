#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
COMPOSE_FILE="${ROOT_DIR}/scripts/tests/docker/affinity_mock_stack.compose.yml"
STAMP="$(date +%Y%m%d%H%M%S)"
TEST_PROJECT="codexmanager-affinity-${STAMP}"
SERVICE_PORT="59760"
WEB_PORT="59761"
KEEP_TEST_STACK="0"
SKIP_DESKTOP_BUILD="0"
PLATFORM_KEY="cm_affinity_test_key"
DATA_VOLUME=""
ENV_FILE=""
NETWORK_NAME=""
PROBE_CONTAINER=""

log() { printf '\n[%s] %s\n' "$(date '+%H:%M:%S')" "$*"; }
die() { printf '\n[ERROR] %s\n' "$*" >&2; exit 1; }
need_cmd() { command -v "$1" >/dev/null 2>&1 || die "missing command: $1"; }

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

for name in CODEX_API_KEY OPENAI_API_KEY OPENAI_API_BASE CODEX_API_BASE; do
  if [[ -n "${!name:-}" ]]; then
    die "refusing to run with ${name} set; mock affinity verification must not use real upstream credentials"
  fi
done

run_desktop_build() {
  docker run --rm \
    -v "${ROOT_DIR}/apps:/src:ro" \
    node:22-bookworm-slim \
    bash -lc "set -euo pipefail && corepack enable >/dev/null 2>&1 && corepack prepare pnpm@10.30.3 --activate >/dev/null 2>&1 && mkdir -p /tmp/apps && cp -a /src/. /tmp/apps/ && rm -rf /tmp/apps/node_modules /tmp/apps/.next /tmp/apps/out && cd /tmp/apps && export NEXT_TELEMETRY_DISABLED=1 CI=true && pnpm install --frozen-lockfile && pnpm run build:desktop"
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
  docker run --rm -i -v "${DATA_VOLUME}:/data" python:3.12-slim python -
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
]:
    cur.execute(f"DELETE FROM {table}")
conn.commit()
conn.close()
PY
}

apply_usage_map() {
  local usage_json="$1"
  docker run --rm -i -e USAGE_JSON="${usage_json}" -v "${DATA_VOLUME}:/data" python:3.12-slim python - <<'PY'
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
conn.commit()
conn.close()
PY
}

set_mock_token() {
  local account_id="$1"
  local token_value="$2"
  docker run --rm -i \
    -e ACCOUNT_ID="${account_id}" \
    -e TOKEN_VALUE="${token_value}" \
    -v "${DATA_VOLUME}:/data" \
    python:3.12-slim python - <<'PY'
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

assert_session_bound_to() {
  local session_id="$1"
  local expected_account="$2"
  local actual
  actual="$(docker run --rm -i -e SESSION_ID="${session_id}" -v "${DATA_VOLUME}:/data" python:3.12-slim python - <<'PY'
import os
import sqlite3

session_id = os.environ["SESSION_ID"]
conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
row = cur.execute(
    "SELECT account_id FROM client_bindings WHERE affinity_key = ? LIMIT 1",
    (f"sid:{session_id}",),
).fetchone()
print("" if row is None else row[0])
conn.close()
PY
)"
  [[ "${actual}" == "${expected_account}" ]] || die "session ${session_id} expected ${expected_account}, got ${actual}"
}

assert_session_unbound() {
  local session_id="$1"
  local actual
  actual="$(docker run --rm -i -e SESSION_ID="${session_id}" -v "${DATA_VOLUME}:/data" python:3.12-slim python - <<'PY'
import os
import sqlite3

session_id = os.environ["SESSION_ID"]
conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
row = cur.execute(
    "SELECT account_id FROM client_bindings WHERE affinity_key = ? LIMIT 1",
    (f"sid:{session_id}",),
).fetchone()
print("" if row is None else row[0])
conn.close()
PY
)"
  [[ -z "${actual}" ]] || die "session ${session_id} expected no binding, got ${actual}"
}

assert_context_events_at_least() {
  local session_id="$1"
  local min_count="$2"
  local actual
  actual="$(docker run --rm -i -e SESSION_ID="${session_id}" -v "${DATA_VOLUME}:/data" python:3.12-slim python - <<'PY'
import os
import sqlite3

session_id = os.environ["SESSION_ID"]
conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
row = cur.execute(
    "SELECT COUNT(1) FROM conversation_context_events WHERE affinity_key = ?",
    (f"sid:{session_id}",),
).fetchone()
print(row[0] if row else 0)
conn.close()
PY
)"
  [[ "${actual}" -ge "${min_count}" ]] || die "session ${session_id} expected at least ${min_count} context events, got ${actual}"
}

assert_context_events_exact() {
  local session_id="$1"
  local expected_count="$2"
  local actual
  actual="$(docker run --rm -i -e SESSION_ID="${session_id}" -v "${DATA_VOLUME}:/data" python:3.12-slim python - <<'PY'
import os
import sqlite3

session_id = os.environ["SESSION_ID"]
conn = sqlite3.connect("/data/codexmanager.db", timeout=5)
cur = conn.cursor()
row = cur.execute(
    "SELECT COUNT(1) FROM conversation_context_events WHERE affinity_key = ?",
    (f"sid:{session_id}",),
).fetchone()
print(row[0] if row else 0)
conn.close()
PY
)"
  [[ "${actual}" == "${expected_count}" ]] || die "session ${session_id} expected exactly ${expected_count} context events, got ${actual}"
}

restart_service() {
  docker compose -p "${TEST_PROJECT}" --env-file "${ENV_FILE}" -f "${COMPOSE_FILE}" restart service-test >/dev/null
  wait_http_ok "http://service-test:48760/health"
  sleep 6
}

send_turn_raw() {
  local session_id="$1"
  local text="$2"
  docker exec -i "${PROBE_CONTAINER}" python - "${PLATFORM_KEY}" "${session_id}" "${text}" <<'PY'
import json
import sys
import urllib.error
import urllib.request

platform_key = sys.argv[1]
session_id = sys.argv[2]
text = sys.argv[3]
request = urllib.request.Request(
    "http://service-test:48760/v1/responses",
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
    headers={
        "Authorization": f"Bearer {platform_key}",
        "Content-Type": "application/json",
        "session_id": session_id,
    },
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

send_turn() {
  local session_id="$1"
  local text="$2"
  local response
  response="$(send_turn_raw "${session_id}" "${text}")"
  local status="${response##*$'\n'}"
  [[ "${status}" == "200" ]] || die "request failed for ${session_id} with status ${status}"
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
  if [[ "${KEEP_TEST_STACK}" == "1" ]]; then
    log "keeping mock stack ${TEST_PROJECT}"
    return
  fi
  docker compose -p "${TEST_PROJECT}" --env-file "${ENV_FILE}" -f "${COMPOSE_FILE}" down -v --remove-orphans >/dev/null 2>&1 || true
  [[ -n "${ENV_FILE}" ]] && rm -f "${ENV_FILE}"
  [[ -n "${DATA_VOLUME}" ]] && docker volume rm "${DATA_VOLUME}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

DATA_VOLUME="${TEST_PROJECT}-data"
NETWORK_NAME="${TEST_PROJECT}_affinity-net"
PROBE_CONTAINER="${TEST_PROJECT}-mock-upstream-1"
docker volume create "${DATA_VOLUME}" >/dev/null
ENV_FILE="$(mktemp "/tmp/codexmanager-affinity-env.XXXXXX")"
cat >"${ENV_FILE}" <<EOF
AFFINITY_TEST_DATA_VOLUME=${DATA_VOLUME}
AFFINITY_SERVICE_PORT=${SERVICE_PORT}
AFFINITY_WEB_PORT=${WEB_PORT}
AFFINITY_TEST_PLATFORM_KEY=${PLATFORM_KEY}
EOF

cd "${ROOT_DIR}"

if [[ "${SKIP_DESKTOP_BUILD}" != "1" ]]; then
  log "running desktop build in container"
  run_desktop_build
fi

log "starting affinity mock stack ${TEST_PROJECT}"
docker compose -p "${TEST_PROJECT}" --env-file "${ENV_FILE}" -f "${COMPOSE_FILE}" up --build -d

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
assert_session_bound_to "case-a-cli-1" "aff-acc-2"
assert_context_events_at_least "case-a-cli-1" 4

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
set_mock_token "aff-acc-1" "mock-account-1-quota"
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":40,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
send_turn "case-d-cli-1" "quota-failover"
assert_binding_counts 'aff-acc-2=1'
assert_session_bound_to "case-d-cli-1" "aff-acc-2"

log "scenario E: challenge response on top candidate falls back to next account"
clear_affinity_state
restore_mock_tokens
set_mock_token "aff-acc-1" "mock-account-1-challenge"
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":40,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
send_turn "case-e-cli-1" "challenge-failover"
assert_binding_counts 'aff-acc-2=1'
assert_session_bound_to "case-e-cli-1" "aff-acc-2"

log "scenario F: upstream 5xx on top candidate falls back to next account"
clear_affinity_state
restore_mock_tokens
set_mock_token "aff-acc-1" "mock-account-1-5xx"
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":40,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
send_turn "case-f-cli-1" "server-failover"
assert_binding_counts 'aff-acc-2=1'
assert_session_bound_to "case-f-cli-1" "aff-acc-2"

log "scenario G: incomplete stream must not commit binding or context"
clear_affinity_state
restore_mock_tokens
set_mock_token "aff-acc-1" "mock-account-1-incomplete"
apply_usage_map '{"aff-acc-1":10,"aff-acc-2":100,"aff-acc-3":100,"aff-acc-4":100,"aff-acc-5":100}'
restart_service
send_turn_raw "case-g-cli-1" "incomplete-stream" >/dev/null
assert_session_unbound "case-g-cli-1"
assert_context_events_exact "case-g-cli-1" 0

docker compose -p "${TEST_PROJECT}" --env-file "${ENV_FILE}" -f "${COMPOSE_FILE}" ps
log "affinity mock stack verification complete"
