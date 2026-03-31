#!/usr/bin/env bash
set -Eeuo pipefail

SEED_ROOT="${CODEX_SEED_ROOT:-/opt/codex-seed}"
SEED_SKILLS_DIR="${SEED_ROOT}/skills"
SEED_INVENTORY_FILE="${SEED_ROOT}/inventory.json"
SEED_VERSION_FILE="${SEED_ROOT}/manifest.version"

CODEX_HOME="${CODEX_HOME:-/root/.codex}"
HOME_DIR="${HOME:-/root}"
DEST_SKILLS_DIR="${CODEX_MANAGED_SKILLS_ROOT:-${HOME_DIR}/.agents/skills}"
LEGACY_SKILLS_DIR="${CODEX_LEGACY_SKILLS_ROOT:-${CODEX_HOME}/skills}"
DEST_VERSION_FILE="${CODEX_HOME}/.skill-bundle-version"
DEST_MANAGED_FILE="${CODEX_HOME}/.skill-bundle-managed.json"
MIGRATION_STATE_FILE="${CODEX_HOME}/.skill-bundle-migration-state.json"
MIGRATION_STATE_NEXT_FILE="${MIGRATION_STATE_FILE}.next"
SYNC_MODE="${CODEX_SKILL_SYNC_MODE:-always}"

log() {
  printf '[skill-sync] %s\n' "$*" >&2
}

json_skill_names() {
  python3 - "$1" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
if not path.exists():
    raise SystemExit(0)

data = json.loads(path.read_text(encoding="utf-8"))
skills = data.get("skills", [])
for entry in skills:
    if isinstance(entry, str):
        print(entry)
        continue
    if isinstance(entry, dict):
        skill_dir = entry.get("directory_name") or entry.get("name")
        if skill_dir:
            print(skill_dir)
PY
}

write_managed_inventory() {
  python3 - "$DEST_MANAGED_FILE" "$@" <<'PY'
import json
import sys
from pathlib import Path

path = Path(sys.argv[1])
skills = sorted(set(sys.argv[2:]))
path.write_text(json.dumps({"skills": skills}, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
PY
}

begin_legacy_migration() {
  python3 - "${LEGACY_SKILLS_DIR}" "${DEST_SKILLS_DIR}" "${DEST_MANAGED_FILE}" "${CODEX_HOME}" "${MIGRATION_STATE_NEXT_FILE}" <<'PY'
import json
import shutil
import sys
from datetime import datetime, timezone
from pathlib import Path

legacy_root = Path(sys.argv[1])
managed_root = Path(sys.argv[2])
managed_file = Path(sys.argv[3])
codex_home = Path(sys.argv[4])
state_path = Path(sys.argv[5])

legacy_root.mkdir(parents=True, exist_ok=True)
managed_root.mkdir(parents=True, exist_ok=True)
codex_home.mkdir(parents=True, exist_ok=True)

managed_names: set[str] = set()
if managed_file.exists():
    payload = json.loads(managed_file.read_text(encoding="utf-8"))
    for entry in payload.get("skills", []):
        if isinstance(entry, str):
            managed_names.add(entry)
        elif isinstance(entry, dict):
            skill_dir = entry.get("directory_name") or entry.get("name")
            if skill_dir:
                managed_names.add(skill_dir)

summary = {
    "timestamp": datetime.now(timezone.utc).isoformat(),
    "legacy_root": str(legacy_root),
    "managed_root": str(managed_root),
    "preserved_entries": [],
    "moved_user_skills": [],
    "conflicts": [],
    "pending_managed_cleanup": sorted(managed_names),
}

conflict_root = codex_home / "legacy-skill-conflicts"

for entry in sorted(legacy_root.iterdir(), key=lambda item: item.name):
    if entry.name == ".system":
        summary["preserved_entries"].append(entry.name)
        continue
    if not entry.is_dir():
        continue
    if entry.name in managed_names:
        continue

    target = managed_root / entry.name
    if not target.exists():
        shutil.move(str(entry), str(target))
        summary["moved_user_skills"].append(entry.name)
        continue

    conflict_root.mkdir(parents=True, exist_ok=True)
    conflict_target = conflict_root / entry.name
    suffix = 1
    while conflict_target.exists():
        conflict_target = conflict_root / f"{entry.name}.{suffix}"
        suffix += 1
    shutil.move(str(entry), str(conflict_target))
    summary["conflicts"].append(
        {
            "name": entry.name,
            "backup_path": str(conflict_target),
        }
    )

state_path.write_text(json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
PY
}

finalize_legacy_migration() {
  python3 - "${MIGRATION_STATE_NEXT_FILE}" "${MIGRATION_STATE_FILE}" "$@" <<'PY'
import json
import sys
from pathlib import Path

source_path = Path(sys.argv[1])
final_path = Path(sys.argv[2])
removed_managed = sorted(set(sys.argv[3:]))

if source_path.exists():
    state = json.loads(source_path.read_text(encoding="utf-8"))
else:
    state = {}

state["removed_managed_after_sync"] = removed_managed
state.pop("pending_managed_cleanup", None)
final_path.write_text(json.dumps(state, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
if source_path.exists():
    source_path.unlink()
PY
  chmod 600 "${MIGRATION_STATE_FILE}"
}

prune_legacy_managed_dirs() {
  local skill_name
  local removed=()

  for skill_name in "$@"; do
    [[ -n "${skill_name}" ]] || continue
    [[ "${skill_name}" == ".system" ]] && continue
    if [[ -d "${LEGACY_SKILLS_DIR}/${skill_name}" ]]; then
      rm -rf -- "${LEGACY_SKILLS_DIR:?}/${skill_name}"
      removed+=("${skill_name}")
    fi
  done

  finalize_legacy_migration "${removed[@]}"
}

main() {
  mkdir -p "${CODEX_HOME}" "${DEST_SKILLS_DIR}" "${LEGACY_SKILLS_DIR}"

  if [[ ! -d "${SEED_SKILLS_DIR}" || ! -f "${SEED_INVENTORY_FILE}" || ! -f "${SEED_VERSION_FILE}" ]]; then
    log "No seeded skill bundle found under ${SEED_ROOT}; skipping sync."
    exit 0
  fi

  local seed_version current_version sync_required
  seed_version="$(tr -d '\r\n' < "${SEED_VERSION_FILE}")"
  current_version=""
  if [[ -f "${DEST_VERSION_FILE}" ]]; then
    current_version="$(tr -d '\r\n' < "${DEST_VERSION_FILE}")"
  fi

  mapfile -t seed_skill_names < <(json_skill_names "${SEED_INVENTORY_FILE}")
  if [[ "${#seed_skill_names[@]}" -eq 0 ]]; then
    log "Seed inventory is empty; skipping sync."
    exit 0
  fi

  mapfile -t old_managed_skill_names < <(json_skill_names "${DEST_MANAGED_FILE}")
  begin_legacy_migration

  sync_required=0
  if [[ "${SYNC_MODE}" == "always" ]]; then
    sync_required=1
  elif [[ "${seed_version}" != "${current_version}" ]]; then
    sync_required=1
  else
    local skill_name
    for skill_name in "${seed_skill_names[@]}"; do
      if [[ ! -f "${DEST_SKILLS_DIR}/${skill_name}/SKILL.md" ]]; then
        sync_required=1
        break
      fi
    done
  fi

  if [[ "${sync_required}" -eq 1 ]]; then
    declare -A seed_skill_map=()
    local skill_name
    for skill_name in "${seed_skill_names[@]}"; do
      seed_skill_map["${skill_name}"]=1
    done

    for skill_name in "${old_managed_skill_names[@]}"; do
      if [[ -z "${seed_skill_map[${skill_name}]:-}" ]]; then
        rm -rf -- "${DEST_SKILLS_DIR:?}/${skill_name}"
      fi
    done

    for skill_name in "${seed_skill_names[@]}"; do
      rm -rf -- "${DEST_SKILLS_DIR:?}/${skill_name}"
      cp -aL "${SEED_SKILLS_DIR}/${skill_name}" "${DEST_SKILLS_DIR}/"
    done

    printf '%s\n' "${seed_version}" > "${DEST_VERSION_FILE}"
    chmod 600 "${DEST_VERSION_FILE}"
    write_managed_inventory "${seed_skill_names[@]}"
    chmod 600 "${DEST_MANAGED_FILE}"
    log "Synchronized ${#seed_skill_names[@]} seeded skills into ${DEST_SKILLS_DIR}."
  else
    log "Seeded skill bundle already synchronized (${seed_version})."
  fi

  prune_legacy_managed_dirs "${old_managed_skill_names[@]}"
}

main "$@"
