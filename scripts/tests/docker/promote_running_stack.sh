#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
COMPOSE_FILE="${ROOT_DIR}/docker/docker-compose.yml"
REMOTE_NAME="origin"
BRANCH_NAME="$(git -C "${ROOT_DIR}" rev-parse --abbrev-ref HEAD)"
PROD_PROJECT="codexmanager"
SKIP_GIT_PUSH="0"

log() { printf '\n[%s] %s\n' "$(date '+%H:%M:%S')" "$*"; }
die() { printf '\n[ERROR] %s\n' "$*" >&2; exit 1; }
need_cmd() { command -v "$1" >/dev/null 2>&1 || die "missing command: $1"; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --compose-file)
      COMPOSE_FILE="$2"
      shift 2
      ;;
    --project)
      PROD_PROJECT="$2"
      shift 2
      ;;
    --remote)
      REMOTE_NAME="$2"
      shift 2
      ;;
    --branch)
      BRANCH_NAME="$2"
      shift 2
      ;;
    --skip-git-push)
      SKIP_GIT_PUSH="1"
      shift
      ;;
    *)
      die "unknown arg: $1"
      ;;
  esac
done

need_cmd git
need_cmd docker
need_cmd curl

if [[ -n "$(git -C "${ROOT_DIR}" status --porcelain --untracked-files=all)" ]]; then
  die "working tree is not clean; commit changes before promotion"
fi

if [[ "${SKIP_GIT_PUSH}" != "1" ]]; then
  log "pushing ${BRANCH_NAME} to ${REMOTE_NAME}"
  git -C "${ROOT_DIR}" push "${REMOTE_NAME}" "${BRANCH_NAME}"
fi

log "restarting production compose project ${PROD_PROJECT}"
docker compose -p "${PROD_PROJECT}" -f "${COMPOSE_FILE}" down
docker compose -p "${PROD_PROJECT}" -f "${COMPOSE_FILE}" up --build -d

log "waiting for production service"
for _ in $(seq 1 60); do
  if curl -fsS "http://127.0.0.1:48760/health" >/dev/null 2>&1; then
    break
  fi
  sleep 2
done
curl -fsS "http://127.0.0.1:48760/health" >/dev/null
curl -fsS "http://127.0.0.1:48761/api/runtime" >/dev/null

log "production stack promotion complete"
