#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
AFFINITY_VERIFY_SCRIPT="${ROOT_DIR}/scripts/tests/docker/verify_affinity_mock_stack.sh"
STAMP="$(date +%Y%m%d%H%M%S)-$$-${RANDOM}"
TEST_PROJECT="codexmanager-cli-child-${STAMP}"
KEEP_TEST_STACK="0"
BUILDER_IMAGE=""

log() { printf '\n[%s] %s\n' "$(date '+%H:%M:%S')" "$*"; }
die() { printf '\n[ERROR] %s\n' "$*" >&2; exit 1; }
need_cmd() { command -v "$1" >/dev/null 2>&1 || die "missing command: $1"; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --test-project)
      TEST_PROJECT="$2"
      shift 2
      ;;
    --keep-test-stack)
      KEEP_TEST_STACK="1"
      shift
      ;;
    *)
      die "unknown arg: $1"
      ;;
  esac
done

cleanup() {
  if [[ -n "${BUILDER_IMAGE}" ]]; then
    docker image rm "${BUILDER_IMAGE}" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

need_cmd docker
need_cmd bash

for name in \
  CODEX_API_KEY \
  OPENAI_API_KEY \
  OPENAI_API_BASE \
  CODEX_API_BASE \
  OPENAI_REFRESH_TOKEN \
  OPENAI_AUTH_COOKIE \
  CODEXMANAGER_AUTH_COOKIE \
  OPENAI_SESSION_COOKIE
do
  if [[ -n "${!name:-}" ]]; then
    die "refusing to run with ${name} set; CLI child-key verification must not use real upstream credentials or cookies"
  fi
done

BUILDER_IMAGE="${TEST_PROJECT}-service-builder"

log "building self-hosted service builder image"
docker build \
  -f "${ROOT_DIR}/docker/Dockerfile.service" \
  --target builder \
  -t "${BUILDER_IMAGE}" \
  "${ROOT_DIR}" >/dev/null

log "running oauth child-key integration test inside self-built container"
docker run --rm "${BUILDER_IMAGE}" bash -lc \
  'set -euo pipefail
   CARGO_BIN="$(command -v cargo || true)"
   if [[ -z "${CARGO_BIN}" && -x /usr/local/cargo/bin/cargo ]]; then
     CARGO_BIN=/usr/local/cargo/bin/cargo
   fi
   if [[ -z "${CARGO_BIN}" && -x /root/.cargo/bin/cargo ]]; then
     CARGO_BIN=/root/.cargo/bin/cargo
   fi
   [[ -n "${CARGO_BIN}" ]] || { echo "cargo not found in builder image" >&2; exit 1; }
   cd /src
   "${CARGO_BIN}" test --locked -p codexmanager-service --test oauth_cli -- --nocapture'

log "running affinity failover regression stack inside self-built containers"
AFFINITY_TEST_PROJECT="${TEST_PROJECT}-affinity"
if [[ "${KEEP_TEST_STACK}" == "1" ]]; then
  bash "${AFFINITY_VERIFY_SCRIPT}" --test-project "${AFFINITY_TEST_PROJECT}" --keep-test-stack
else
  bash "${AFFINITY_VERIFY_SCRIPT}" --test-project "${AFFINITY_TEST_PROJECT}"
fi

log "cli child-key verification complete"
