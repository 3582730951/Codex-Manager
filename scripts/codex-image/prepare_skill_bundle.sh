#!/usr/bin/env bash
set -Eeuo pipefail

CODEX_HOME="${CODEX_HOME:-/root/.codex}"
CODEX_IMAGE_TOOL_ROOT="${CODEX_IMAGE_TOOL_ROOT:-/opt/codex-image}"
CODEX_SKILL_MANIFEST="${CODEX_SKILL_MANIFEST:-${CODEX_IMAGE_TOOL_ROOT}/skill_manifest.lock.json}"
IMAGE_SEED_ROOT="${IMAGE_SEED_ROOT:-/opt/codex-seed}"
SKILL_UPDATE_MODE="${CODEX_SKILL_UPDATE_MODE:-auto}"

CACHE_ROOT="${CODEX_HOME}/.skill-bundle-cache"
REMOTE_WORK_DIR="${CACHE_ROOT}/work"
REMOTE_SEED_ROOT="${CACHE_ROOT}/remote-seed"
REMOTE_NEXT_ROOT="${CACHE_ROOT}/remote-seed.next"

log() {
  printf '[skill-prepare] %s\n' "$*" >&2
}

seed_root_is_valid() {
  local seed_root="$1"
  [[ -d "${seed_root}/skills" && -f "${seed_root}/inventory.json" && -f "${seed_root}/manifest.version" ]]
}

build_remote_seed() {
  mkdir -p "${CACHE_ROOT}" "${REMOTE_WORK_DIR}"
  rm -rf "${REMOTE_NEXT_ROOT}"

  if ! python3 "${CODEX_IMAGE_TOOL_ROOT}/install_skill_bundle.py" \
    --manifest "${CODEX_SKILL_MANIFEST}" \
    --output-dir "${REMOTE_NEXT_ROOT}" \
    --work-dir "${REMOTE_WORK_DIR}" \
    --resolve-heads; then
    rm -rf "${REMOTE_NEXT_ROOT}"
    return 1
  fi

  rm -rf "${REMOTE_SEED_ROOT}"
  mv "${REMOTE_NEXT_ROOT}" "${REMOTE_SEED_ROOT}"
  printf '%s\n' "${REMOTE_SEED_ROOT}"
}

main() {
  case "${SKILL_UPDATE_MODE}" in
    image)
      seed_root_is_valid "${IMAGE_SEED_ROOT}" || {
        log "Image seed root is invalid: ${IMAGE_SEED_ROOT}"
        exit 1
      }
      log "Using image-seeded skill bundle."
      printf '%s\n' "${IMAGE_SEED_ROOT}"
      ;;
    auto|remote)
      if remote_seed="$(build_remote_seed)"; then
        log "Prepared remote-updated skill bundle at ${remote_seed}."
        printf '%s\n' "${remote_seed}"
        exit 0
      fi

      if seed_root_is_valid "${REMOTE_SEED_ROOT}"; then
        log "Remote refresh failed; falling back to cached remote bundle ${REMOTE_SEED_ROOT}."
        printf '%s\n' "${REMOTE_SEED_ROOT}"
        exit 0
      fi

      if [[ "${SKILL_UPDATE_MODE}" == "auto" ]] && seed_root_is_valid "${IMAGE_SEED_ROOT}"; then
        log "Remote refresh failed; falling back to image bundle ${IMAGE_SEED_ROOT}."
        printf '%s\n' "${IMAGE_SEED_ROOT}"
        exit 0
      fi

      log "No usable skill bundle available for mode ${SKILL_UPDATE_MODE}."
      exit 1
      ;;
    *)
      log "Unsupported CODEX_SKILL_UPDATE_MODE: ${SKILL_UPDATE_MODE}"
      exit 1
      ;;
  esac
}

main "$@"
