#!/usr/bin/env bash
set -Eeuo pipefail

export HOME="${HOME:-/root}"
export CODEX_HOME="${CODEX_HOME:-${HOME}/.codex}"

REAL_CODEX_BIN="${REAL_CODEX_BIN:-/usr/local/bin/codex-real}"
CODEXMANAGER_OAUTH_META_FILE="${CODEX_HOME}/codexmanager-oauth.json"
CODEXMANAGER_OAUTH_LOGIN_HELPER="${CODEXMANAGER_OAUTH_LOGIN_HELPER:-/usr/local/bin/codex-oauth-proxy-login}"

sync_openai_api_key_from_auth_file() {
  [[ -z "${OPENAI_API_KEY:-}" ]] || return 0

  local loaded_key=""
  loaded_key="$(
    python3 - <<'EOF_AUTH_ENV'
import json
import os
from pathlib import Path

auth_path = Path(os.environ.get("CODEX_HOME", "/root/.codex")) / "auth.json"
if not auth_path.exists():
    raise SystemExit(0)

try:
    payload = json.loads(auth_path.read_text(encoding="utf-8"))
except Exception:
    raise SystemExit(0)

if payload.get("auth_mode") != "apikey":
    raise SystemExit(0)

api_key = str(payload.get("OPENAI_API_KEY", "")).strip()
if api_key:
    print(api_key)
EOF_AUTH_ENV
  )"

  if [[ -n "${loaded_key}" ]]; then
    export OPENAI_API_KEY="${loaded_key}"
  fi
}

proxy_login_enabled() {
  [[ "${CODEX_AUTH_MODE:-}" == "oauth" ]] || return 1
  [[ -n "${CODEX_OAUTH_BROWSER_ISSUER_BASE_URL:-}" ]] || return 1
  [[ -x "${CODEXMANAGER_OAUTH_LOGIN_HELPER}" ]] || return 1
}

print_proxy_status() {
  python3 - <<'EOF_STATUS'
import json
import os
from pathlib import Path

codex_home = Path(os.environ.get("CODEX_HOME", "/root/.codex"))
meta_path = codex_home / "codexmanager-oauth.json"
auth_path = codex_home / "auth.json"

if not meta_path.exists() or not auth_path.exists():
    print("Not logged in")
    raise SystemExit(0)

meta = json.loads(meta_path.read_text(encoding="utf-8"))
print("Logged in using CodexManager OAuth proxy")
print(f"Browser issuer: {meta.get('browser_issuer_base_url', '')}")
print(f"Token issuer: {meta.get('token_issuer_base_url', '')}")
print(f"Callback URL: {meta.get('callback_public_url', '')}")
EOF_STATUS
}

if [[ $# -gt 0 && "$1" == "login" ]] && proxy_login_enabled; then
  shift
  case "${1:-}" in
    "")
      exec "${CODEXMANAGER_OAUTH_LOGIN_HELPER}"
      ;;
    status)
      if [[ -f "${CODEXMANAGER_OAUTH_META_FILE}" ]]; then
        print_proxy_status
        exit 0
      fi
      exec "${REAL_CODEX_BIN}" login status
      ;;
    help|-h|--help)
      exec "${REAL_CODEX_BIN}" login "$@"
      ;;
    *)
      exec "${REAL_CODEX_BIN}" login "$@"
      ;;
  esac
fi

if [[ $# -gt 0 && "$1" == "logout" ]]; then
  rm -f "${CODEXMANAGER_OAUTH_META_FILE}" || true
fi

sync_openai_api_key_from_auth_file

exec "${REAL_CODEX_BIN}" "$@"
