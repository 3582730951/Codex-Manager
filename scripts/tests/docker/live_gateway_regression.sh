#!/usr/bin/env bash
set -Eeuo pipefail

BASE="http://127.0.0.1:48760"
API_KEY=""
MODEL="gpt-5.3-codex"
TIMEOUT_SECONDS="120"
TMP_DIR=""

TOOL_NAME="mcp__tool_server_namespace_for_codex_manager_gateway_adapter_alignment__very_long_tool_operation_name"

log() { printf '\n[%s] %s\n' "$(date '+%H:%M:%S')" "$*"; }
die() { printf '\n[ERROR] %s\n' "$*" >&2; exit 1; }
need_cmd() { command -v "$1" >/dev/null 2>&1 || die "missing command: $1"; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --base)
      BASE="$2"
      shift 2
      ;;
    --api-key)
      API_KEY="$2"
      shift 2
      ;;
    --model)
      MODEL="$2"
      shift 2
      ;;
    --timeout-seconds)
      TIMEOUT_SECONDS="$2"
      shift 2
      ;;
    *)
      die "unknown arg: $1"
      ;;
  esac
done

[[ -n "${API_KEY}" ]] || die "missing --api-key"
need_cmd curl
need_cmd python3

TMP_DIR="$(mktemp -d)"
cleanup() {
  rm -rf "${TMP_DIR}" || true
}
trap cleanup EXIT

make_chat_body() {
  local stream_flag="$1"
  local out_file="$2"
  cat >"${out_file}" <<JSON
{"model":"${MODEL}","stream":${stream_flag},"messages":[{"role":"user","content":"Call the specified tool with {\"path\":\"README.md\"} and return tool call."}],"tools":[{"type":"function","function":{"name":"${TOOL_NAME}","description":"Read file by path","parameters":{"type":"object","properties":{"path":{"type":"string","description":"file path"}},"required":["path"]}}}],"tool_choice":{"type":"function","function":{"name":"${TOOL_NAME}"}}}
JSON
}

assert_http_200() {
  local header_file="$1"
  local status
  status="$(awk 'toupper($1) ~ /^HTTP\// { code=$2 } END { print code }' "${header_file}")"
  [[ "${status}" == "200" ]] || die "unexpected status ${status} from ${header_file}"
}

run_non_stream_probe() {
  local body_file="${TMP_DIR}/chat-non-stream.json"
  local headers_file="${TMP_DIR}/chat-non-stream.headers"
  local response_file="${TMP_DIR}/chat-non-stream.json.out"
  make_chat_body "false" "${body_file}"

  log "running non-stream chat tools probe"
  curl -sS \
    --max-time "${TIMEOUT_SECONDS}" \
    -D "${headers_file}" \
    -o "${response_file}" \
    -X POST "${BASE%/}/v1/chat/completions" \
    -H "Authorization: Bearer ${API_KEY}" \
    -H "Content-Type: application/json" \
    --data-binary "@${body_file}"
  assert_http_200 "${headers_file}"

  python3 - "${response_file}" "${TOOL_NAME}" <<'PY'
import json
import sys

path, tool_name = sys.argv[1], sys.argv[2]
with open(path, "r", encoding="utf-8") as fh:
    payload = json.load(fh)

choices = payload.get("choices") or []
if not choices:
    raise SystemExit("missing choices in non-stream response")

choice = choices[0]
if choice.get("finish_reason") != "tool_calls":
    raise SystemExit(f"unexpected finish_reason: {choice.get('finish_reason')!r}")

message = choice.get("message") or {}
tool_calls = message.get("tool_calls") or []
if not tool_calls:
    raise SystemExit("missing tool_calls in non-stream response")

fn = (tool_calls[0].get("function") or {})
if fn.get("name") != tool_name:
    raise SystemExit(f"unexpected tool name: {fn.get('name')!r}")

arguments = fn.get("arguments") or ""
if '"path":"README.md"' not in arguments and '"path": "README.md"' not in arguments:
    raise SystemExit(f"unexpected tool arguments: {arguments!r}")
PY
}

run_stream_probe() {
  local body_file="${TMP_DIR}/chat-stream.json"
  local headers_file="${TMP_DIR}/chat-stream.headers"
  local response_file="${TMP_DIR}/chat-stream.txt"
  make_chat_body "true" "${body_file}"

  log "running stream chat tools probe"
  curl -sS -N \
    --max-time "${TIMEOUT_SECONDS}" \
    -D "${headers_file}" \
    -o "${response_file}" \
    -X POST "${BASE%/}/v1/chat/completions" \
    -H "Authorization: Bearer ${API_KEY}" \
    -H "Content-Type: application/json" \
    --data-binary "@${body_file}"
  assert_http_200 "${headers_file}"

  python3 - "${response_file}" "${TOOL_NAME}" <<'PY'
import json
import sys

path, tool_name = sys.argv[1], sys.argv[2]
tool_seen = False
finish_seen = False
usage_seen = False
done_seen = False

with open(path, "r", encoding="utf-8") as fh:
    for raw_line in fh:
        line = raw_line.strip()
        if not line.startswith("data:"):
            continue
        payload = line[5:].strip()
        if not payload:
            continue
        if payload == "[DONE]":
            done_seen = True
            continue
        obj = json.loads(payload)
        if obj.get("usage") is not None:
            usage_seen = True
        choices = obj.get("choices") or []
        if not choices:
            continue
        choice = choices[0]
        if choice.get("finish_reason") == "tool_calls":
            finish_seen = True
        delta = choice.get("delta") or {}
        tool_calls = delta.get("tool_calls") or []
        if not tool_calls:
            continue
        fn = (tool_calls[0].get("function") or {})
        if fn.get("name") == tool_name:
            tool_seen = True

if not tool_seen:
    raise SystemExit("missing streamed tool call")
if not finish_seen:
    raise SystemExit("missing streamed finish_reason=tool_calls")
if not usage_seen:
    raise SystemExit("missing streamed usage frame")
if not done_seen:
    raise SystemExit("missing streamed [DONE] marker")
PY
}

run_non_stream_probe
run_stream_probe
log "live gateway regression passed"
