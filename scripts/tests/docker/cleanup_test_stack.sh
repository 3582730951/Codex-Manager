#!/usr/bin/env bash
set -Eeuo pipefail

PROJECT_NAME=""
COMPOSE_FILE=""
OVERRIDE_FILE=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --project)
      PROJECT_NAME="$2"
      shift 2
      ;;
    --compose-file)
      COMPOSE_FILE="$2"
      shift 2
      ;;
    --override-file)
      OVERRIDE_FILE="$2"
      shift 2
      ;;
    *)
      echo "unknown arg: $1" >&2
      exit 1
      ;;
  esac
done

[[ -n "${PROJECT_NAME}" ]] || { echo "missing --project" >&2; exit 1; }
[[ -n "${COMPOSE_FILE}" ]] || { echo "missing --compose-file" >&2; exit 1; }

if command -v docker >/dev/null 2>&1; then
  if [[ -n "${OVERRIDE_FILE}" && -f "${OVERRIDE_FILE}" ]]; then
    docker compose -p "${PROJECT_NAME}" -f "${COMPOSE_FILE}" -f "${OVERRIDE_FILE}" down -v --remove-orphans --rmi local >/dev/null 2>&1 || true
  else
    docker compose -p "${PROJECT_NAME}" -f "${COMPOSE_FILE}" down -v --remove-orphans --rmi local >/dev/null 2>&1 || true
  fi
fi

if [[ -n "${OVERRIDE_FILE}" && -f "${OVERRIDE_FILE}" ]]; then
  rm -f "${OVERRIDE_FILE}"
fi
