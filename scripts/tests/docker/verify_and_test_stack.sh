#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
COMPOSE_FILE="${ROOT_DIR}/docker/docker-compose.yml"
STAMP="$(date +%Y%m%d%H%M%S)"
TEST_PROJECT="codexmanager-test-${STAMP}"
SERVICE_PORT="58760"
WEB_PORT="58761"
SKIP_LIVE_GATEWAY="0"
KEEP_TEST_STACK="0"
SEED_CONTAINER=""
KEY_CONTAINER=""

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
    --skip-live-gateway)
      SKIP_LIVE_GATEWAY="1"
      shift
      ;;
    --keep-test-stack)
      KEEP_TEST_STACK="1"
      shift
      ;;
    --seed-container)
      SEED_CONTAINER="$2"
      shift 2
      ;;
    --key-container)
      KEY_CONTAINER="$2"
      shift 2
      ;;
    *)
      die "unknown arg: $1"
      ;;
  esac
done

need_cmd docker
need_cmd curl

resolve_gateway_api_key() {
  if [[ -n "${CODEX_API_KEY:-}" ]]; then
    printf '%s' "${CODEX_API_KEY}"
    return
  fi
  if [[ -n "${OPENAI_API_KEY:-}" ]]; then
    printf '%s' "${OPENAI_API_KEY}"
    return
  fi

  local candidate="${KEY_CONTAINER}"
  if [[ -z "${candidate}" ]]; then
    candidate="$(docker ps --format '{{.Names}}' | grep '^codex-' | head -n 1 || true)"
  fi
  [[ -n "${candidate}" ]] || die "missing CODEX_API_KEY/OPENAI_API_KEY and no key container found"

  local key
  key="$(docker inspect "${candidate}" --format '{{range .Config.Env}}{{println .}}{{end}}' | sed -n 's/^CODEX_API_KEY=//p' | head -n 1)"
  if [[ -z "${key}" ]]; then
    key="$(docker inspect "${candidate}" --format '{{range .Config.Env}}{{println .}}{{end}}' | sed -n 's/^OPENAI_API_KEY=//p' | head -n 1)"
  fi
  [[ -n "${key}" ]] || die "no CODEX_API_KEY/OPENAI_API_KEY found in container ${candidate}"
  printf '%s' "${key}"
}

resolve_seed_container() {
  if [[ -n "${SEED_CONTAINER}" ]]; then
    printf '%s' "${SEED_CONTAINER}"
    return
  fi
  docker ps --format '{{.Names}}' | grep '^codexmanager.*service' | head -n 1 || true
}

run_cargo_tests() {
  if command -v cargo >/dev/null 2>&1; then
    cargo test -p codexmanager-service --locked
    return
  fi
  docker run --rm \
    -v "${ROOT_DIR}:/workspace:ro" \
    -w /workspace \
    rust:1-bookworm \
    bash -c "export CARGO_HOME=/tmp/cargo-home CARGO_TARGET_DIR=/tmp/cargo-target && cargo test -p codexmanager-service --locked"
}

run_desktop_build() {
  if command -v pnpm >/dev/null 2>&1; then
    pnpm -C apps install --frozen-lockfile
    pnpm -C apps run build:desktop
    return
  fi
  docker run --rm \
    -v "${ROOT_DIR}/apps:/src:ro" \
    node:22-bookworm-slim \
    bash -c "set -euo pipefail && corepack enable && corepack prepare pnpm@10.30.3 --activate && mkdir -p /tmp/apps && cp -a /src/. /tmp/apps/ && rm -rf /tmp/apps/node_modules /tmp/apps/.next /tmp/apps/out && cd /tmp/apps && export NEXT_TELEMETRY_DISABLED=1 CI=true && pnpm install --frozen-lockfile && pnpm run build:desktop"
}

TEST_COMPOSE_FILE="$(mktemp "${ROOT_DIR}/docker/test-compose.XXXXXX.yml")"
SEED_DIR=""

if [[ -z "${KEY_CONTAINER}" ]]; then
  KEY_CONTAINER="$(docker ps --format '{{.Names}}' | grep '^codex-' | head -n 1 || true)"
fi

if [[ "${SKIP_LIVE_GATEWAY}" != "1" ]]; then
  SEED_CONTAINER="$(resolve_seed_container)"
  [[ -n "${SEED_CONTAINER}" ]] || die "no running codexmanager service container found to seed live gateway regression"
  SEED_DIR="$(mktemp -d)"
  log "seeding test data from ${SEED_CONTAINER}"
  docker cp "${SEED_CONTAINER}:/data/." "${SEED_DIR}/"
  chmod -R a+rwX "${SEED_DIR}"
fi

if [[ -n "${SEED_DIR}" ]]; then
  sed \
    -e "s#\"48760:48760\"#\"${SERVICE_PORT}:48760\"#" \
    -e "s#\"48761:48761\"#\"${WEB_PORT}:48761\"#" \
    -e "s#- codexmanager-data:/data#- ${SEED_DIR}:/data#g" \
    "${COMPOSE_FILE}" >"${TEST_COMPOSE_FILE}"
else
  sed \
    -e "s#\"48760:48760\"#\"${SERVICE_PORT}:48760\"#" \
    -e "s#\"48761:48761\"#\"${WEB_PORT}:48761\"#" \
    "${COMPOSE_FILE}" >"${TEST_COMPOSE_FILE}"
fi

cleanup() {
  if [[ "${KEEP_TEST_STACK}" == "1" ]]; then
    log "keeping test stack ${TEST_PROJECT}"
    return
  fi
  "${ROOT_DIR}/scripts/tests/docker/cleanup_test_stack.sh" \
    --project "${TEST_PROJECT}" \
    --compose-file "${TEST_COMPOSE_FILE}"
  rm -f "${TEST_COMPOSE_FILE}"
  if [[ -n "${SEED_DIR}" ]]; then
    rm -rf "${SEED_DIR}"
  fi
}
trap cleanup EXIT

cd "${ROOT_DIR}"

log "running cargo test"
run_cargo_tests

log "running desktop build"
run_desktop_build

log "starting docker test stack ${TEST_PROJECT}"
docker compose -p "${TEST_PROJECT}" -f "${TEST_COMPOSE_FILE}" up --build -d

log "waiting for service health"
for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:${SERVICE_PORT}/health" >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
curl -fsS "http://127.0.0.1:${SERVICE_PORT}/health" >/dev/null

log "waiting for web runtime endpoint"
for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:${WEB_PORT}/api/runtime" >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
curl -fsS "http://127.0.0.1:${WEB_PORT}/api/runtime" >/dev/null

if [[ "${SKIP_LIVE_GATEWAY}" != "1" ]]; then
  API_KEY="$(resolve_gateway_api_key)"
  log "running gateway regression suite against test stack"
  bash "${ROOT_DIR}/scripts/tests/docker/live_gateway_regression.sh" \
    --base "http://127.0.0.1:${SERVICE_PORT}" \
    --api-key "${API_KEY}"
fi

docker compose -p "${TEST_PROJECT}" -f "${TEST_COMPOSE_FILE}" ps
log "test stack verification complete"
