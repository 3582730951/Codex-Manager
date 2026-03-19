#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
COMPOSE_DIR="$ROOT/crates/docker"
COMPOSE_FILE="$COMPOSE_DIR/docker-compose.yml"
PROJECT_NAME="codexmanager"

SERVICE_NAME="codexmanager-service"
CONTAINER_NAME="codexmanager-codexmanager-service-1"
SERVICE_BASE="http://127.0.0.1:48760"
TRACE_PATH="/data/gateway-trace.log"
REBUILD=false

usage() {
  cat <<'EOF'
Usage:
  scripts/tests/docker_gateway_probe.sh [--rebuild] [--project <name>] [--container <name>] [--base <url>]

Options:
  --rebuild           Rebuild and restart the service container before probing
  --project <name>    Override the docker compose project name
  --container <name>  Override the service container name
  --base <url>        Override the host-visible service base URL
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --rebuild)
      REBUILD=true
      shift
      ;;
    --project)
      PROJECT_NAME="${2:-}"
      shift 2
      ;;
    --container)
      CONTAINER_NAME="${2:-}"
      shift 2
      ;;
    --base)
      SERVICE_BASE="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

step() {
  echo
  echo "==> $*"
}

wait_for_service() {
  local attempt=1
  local max_attempts=15
  while (( attempt <= max_attempts )); do
    if curl -fsS "$SERVICE_BASE/" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
    attempt=$((attempt + 1))
  done
  echo "service probe failed after ${max_attempts}s: $SERVICE_BASE/" >&2
  return 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "missing command: $1" >&2
    exit 1
  }
}

require_cmd docker
require_cmd curl

if [[ "$REBUILD" == "true" ]]; then
  step "Rebuild and restart $SERVICE_NAME"
  docker compose -p "$PROJECT_NAME" -f "$COMPOSE_FILE" up -d --build "$SERVICE_NAME"
fi

step "Container status"
docker ps --filter "name=$CONTAINER_NAME"

step "Health probe"
wait_for_service
curl -fsS "$SERVICE_BASE/"

step "Metrics snapshot"
METRICS="$(curl -fsS "$SERVICE_BASE/metrics")"
printf '%s\n' "$METRICS" | grep 'codexmanager_gateway_requests_labeled_total' || true
printf '%s\n' "$METRICS" | grep 'codexmanager_http_queue_enqueue_failures_total' || true
printf '%s\n' "$METRICS" | grep 'codexmanager_gateway_upstream_attempt_errors_total' || true

step "Service env"
docker exec "$CONTAINER_NAME" sh -lc \
  'printenv | grep "^CODEXMANAGER_" | sort || true'

step "Trace tail"
docker exec "$CONTAINER_NAME" sh -lc \
  "if [ -f '$TRACE_PATH' ]; then tail -n 120 '$TRACE_PATH'; else echo 'trace file not found: $TRACE_PATH'; fi"

step "Trace counters"
docker exec "$CONTAINER_NAME" sh -lc "
  if [ ! -f '$TRACE_PATH' ]; then
    echo 'trace file not found: $TRACE_PATH'
    exit 0
  fi
  for key in gate_rejected fallback_not_configured upstream_challenge_blocked upstream_rate_limited gateway_bridge_error upstream_stream_idle_timeout; do
    count=\$(grep -c \"\$key\" '$TRACE_PATH' || true)
    printf '%s=%s\n' \"\$key\" \"\$count\"
  done
"
