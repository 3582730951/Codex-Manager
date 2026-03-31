#!/usr/bin/env bash
set -Eeuo pipefail

SEED_ROOT="${CODEX_SEED_ROOT:-/opt/codex-seed}"
SEED_INVENTORY_FILE="${SEED_ROOT}/inventory.json"
SEED_VERSION_FILE="${SEED_ROOT}/manifest.version"
HOME_DIR="${HOME:-/root}"
MANAGED_SKILLS_DIR="${CODEX_MANAGED_SKILLS_ROOT:-${HOME_DIR}/.agents/skills}"
LEGACY_SKILLS_DIR="${CODEX_LEGACY_SKILLS_ROOT:-/root/.codex/skills}"
SYSTEM_SKILLS_DIR="${LEGACY_SKILLS_DIR}/.system"
VERIFY_CWD="${CODEX_VERIFY_CWD:-/workspace}"
MIN_SKILL_COUNT="${CODEX_MIN_SKILL_COUNT:-100}"

fail() {
  printf '[verify-skill-bundle] %s\n' "$*" >&2
  exit 1
}

require_file() {
  [[ -f "$1" ]] || fail "Missing file: $1"
}

require_dir() {
  [[ -d "$1" ]] || fail "Missing directory: $1"
}

main() {
  require_dir "${SEED_ROOT}/skills"
  require_file "${SEED_INVENTORY_FILE}"
  require_file "${SEED_VERSION_FILE}"
  require_dir "${MANAGED_SKILLS_DIR}"
  require_dir "${LEGACY_SKILLS_DIR}"
  require_dir "${VERIFY_CWD}"

  python3 - "${SEED_INVENTORY_FILE}" "${MANAGED_SKILLS_DIR}" "${LEGACY_SKILLS_DIR}" "${SYSTEM_SKILLS_DIR}" "${VERIFY_CWD}" "${MIN_SKILL_COUNT}" <<'PY'
import json
import os
import select
import subprocess
import sys
import time
from pathlib import Path

inventory_path = Path(sys.argv[1])
managed_root = Path(sys.argv[2])
legacy_root = Path(sys.argv[3])
system_root = Path(sys.argv[4])
verify_cwd = Path(sys.argv[5])
min_skill_count = int(sys.argv[6])

required = {
    "doc",
    "playwright",
    "screenshot",
    "security-ownership-map",
    "slides",
    "spreadsheet",
    "ui-ux-pro-max",
    "plan-mode-pm-orchestrator",
    "multi-agent-plan-orchestrator",
}
expected_system_skills = {
    "imagegen",
    "openai-docs",
    "plugin-creator",
    "skill-creator",
    "skill-installer",
}


def fail(message: str) -> None:
    raise SystemExit(message)


def ensure_no_symlinks(root: Path, label: str) -> None:
    for current_root, dirnames, filenames in os.walk(root):
        current_path = Path(current_root)
        for name in dirnames + filenames:
            candidate = current_path / name
            if candidate.is_symlink():
                fail(f"Symlink detected inside {label}: {candidate}")


def ensure_under(path: Path, root: Path, label: str) -> None:
    try:
        path.resolve().relative_to(root.resolve())
    except ValueError:
        fail(f"{label} path {path} is not under {root}")


def rpc_request(proc: subprocess.Popen[str], payload: dict[str, object]) -> dict[str, object]:
    assert proc.stdin is not None
    assert proc.stdout is not None

    proc.stdin.write(json.dumps(payload, ensure_ascii=False) + "\n")
    proc.stdin.flush()

    deadline = time.time() + 60
    while time.time() < deadline:
        if proc.poll() is not None:
            stderr = ""
            if proc.stderr is not None:
                stderr = proc.stderr.read()
            fail(f"codex app-server exited early while waiting for {payload['method']}: {stderr.strip()}")

        ready, _, _ = select.select([proc.stdout], [], [], 1)
        if not ready:
            continue

        line = proc.stdout.readline()
        if not line:
            continue

        message = json.loads(line)
        if message.get("id") != payload["id"]:
            continue
        if "error" in message:
            fail(f"JSON-RPC error from {payload['method']}: {message['error']}")
        result = message.get("result")
        if not isinstance(result, dict):
            fail(f"Unexpected JSON-RPC result from {payload['method']}: {message}")
        return result

    fail(f"Timed out waiting for JSON-RPC response to {payload['method']}")


inventory = json.loads(inventory_path.read_text(encoding="utf-8"))
inventory_entries = inventory.get("skills", [])
inventory_by_skill_name: dict[str, dict[str, object]] = {}
inventory_directory_names: set[str] = set()

for entry in inventory_entries:
    if not isinstance(entry, dict):
        fail(f"Invalid inventory entry: {entry!r}")
    skill_name = str(entry.get("skill_name") or entry.get("name") or "").strip()
    directory_name = str(entry.get("directory_name") or entry.get("name") or "").strip()
    if not skill_name:
        fail(f"Inventory entry is missing skill_name: {entry}")
    if not directory_name:
        fail(f"Inventory entry is missing directory_name: {entry}")
    if skill_name in inventory_by_skill_name:
        fail(f"Duplicate inventory skill_name detected: {skill_name}")
    inventory_by_skill_name[skill_name] = entry
    inventory_directory_names.add(directory_name)

if len(inventory_by_skill_name) < min_skill_count:
    fail(
        f"Inventory skill count {len(inventory_by_skill_name)} is below the required minimum of {min_skill_count}."
    )

missing_inventory = sorted(required - inventory_by_skill_name.keys())
if missing_inventory:
    fail("Required skills missing from inventory: " + ", ".join(missing_inventory))

for directory_name in sorted(inventory_directory_names):
    seed_skill_file = inventory_path.parent / "skills" / directory_name / "SKILL.md"
    managed_skill_file = managed_root / directory_name / "SKILL.md"
    if not seed_skill_file.is_file():
        fail(f"Seeded skill missing from bundle: {directory_name}")
    if not managed_skill_file.is_file():
        fail(f"Managed skill missing from live user scope root: {directory_name}")

ensure_no_symlinks(inventory_path.parent / "skills", "seeded skills")
ensure_no_symlinks(managed_root, "managed skills")

proc = subprocess.Popen(
    ["codex", "app-server"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    cwd=str(verify_cwd),
    env={
        **os.environ,
        "RUST_LOG": os.environ.get("RUST_LOG", "error"),
    },
)

try:
    rpc_request(
        proc,
        {
            "id": 1,
            "method": "initialize",
            "params": {
                "clientInfo": {
                    "name": "codex_image_verify",
                    "title": "Codex Image Verify",
                    "version": "1.0.0",
                }
            },
        },
    )

    assert proc.stdin is not None
    proc.stdin.write(json.dumps({"method": "initialized", "params": {}}, ensure_ascii=False) + "\n")
    proc.stdin.flush()

    skills_result = rpc_request(
        proc,
        {
            "id": 2,
            "method": "skills/list",
            "params": {
                "cwds": [str(verify_cwd)],
                "forceReload": True,
            },
        },
    )
finally:
    if proc.stdin is not None:
        proc.stdin.close()
    try:
        proc.terminate()
        proc.wait(timeout=10)
    except Exception:
        proc.kill()
        proc.wait(timeout=10)

data = skills_result.get("data")
if not isinstance(data, list) or not data:
    fail(f"skills/list returned no data: {skills_result}")

selected_entry = None
for entry in data:
    if entry.get("cwd") == str(verify_cwd):
        selected_entry = entry
        break
if selected_entry is None:
    selected_entry = data[0]

errors = selected_entry.get("errors") or []
if errors:
    fail(f"skills/list returned loader errors: {errors}")

listed_skills = selected_entry.get("skills")
if not isinstance(listed_skills, list):
    fail(f"skills/list returned invalid skills payload: {selected_entry}")

skills_by_name: dict[str, dict[str, object]] = {}
duplicate_names: list[str] = []
for item in listed_skills:
    if not isinstance(item, dict):
        fail(f"Invalid skills/list item: {item!r}")
    name = str(item.get("name") or "").strip()
    if not name:
        fail(f"skills/list item is missing name: {item}")
    if name in skills_by_name:
        duplicate_names.append(name)
        continue
    skills_by_name[name] = item

if duplicate_names:
    fail("skills/list returned duplicate skill names: " + ", ".join(sorted(set(duplicate_names))))

missing_live = sorted(required - skills_by_name.keys())
if missing_live:
    fail("Required skills missing from skills/list: " + ", ".join(missing_live))

missing_managed = sorted(set(inventory_by_skill_name) - set(skills_by_name))
if missing_managed:
    fail("Managed bundle skills missing from skills/list: " + ", ".join(missing_managed))

for skill_name in sorted(inventory_by_skill_name):
    listed = skills_by_name[skill_name]
    path = Path(str(listed["path"]))
    ensure_under(path, managed_root, f"Managed skill {skill_name}")

for skill_name in sorted(expected_system_skills):
    listed = skills_by_name.get(skill_name)
    if listed is None:
        fail(f"Expected system skill missing from skills/list: {skill_name}")
    path = Path(str(listed["path"]))
    ensure_under(path, system_root, f"System skill {skill_name}")

if not system_root.is_dir():
    fail(f"System skill cache root does not exist after skills/list: {system_root}")

legacy_entries = sorted(entry.name for entry in legacy_root.iterdir())
unexpected_legacy_entries = [entry for entry in legacy_entries if entry != ".system"]
if unexpected_legacy_entries:
    fail(
        "Legacy skills root still contains non-system entries: "
        + ", ".join(unexpected_legacy_entries)
    )

print(
    "Verified "
    f"{len(inventory_by_skill_name)} managed skills and {len(skills_by_name)} total discovered skills; "
    f"manifest={inventory.get('manifest_version', 'unknown')}; managed_root={managed_root}; system_root={system_root}",
    flush=True,
)
PY
}

main "$@"
