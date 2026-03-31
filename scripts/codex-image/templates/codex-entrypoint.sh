#!/usr/bin/env bash
set -Eeuo pipefail

export HOME="${HOME:-/root}"
export CODEX_HOME="${CODEX_HOME:-/root/.codex}"
export CODEX_MANAGED_SKILLS_ROOT="${CODEX_MANAGED_SKILLS_ROOT:-${HOME}/.agents/skills}"
export CODEX_LEGACY_SKILLS_ROOT="${CODEX_LEGACY_SKILLS_ROOT:-${CODEX_HOME}/skills}"

mkdir -p "${CODEX_HOME}" "${HOME}/.agents" "${CODEX_MANAGED_SKILLS_ROOT}" "${CODEX_LEGACY_SKILLS_ROOT}"

if [[ -n "${OPENAI_API_BASE:-}" ]]; then
  python3 - <<'EOF_CONFIG'
import os
from pathlib import Path

config_path = Path(os.environ.get("CODEX_HOME", "/root/.codex")) / "config.toml"
config_path.parent.mkdir(parents=True, exist_ok=True)
existing = config_path.read_text(encoding="utf-8") if config_path.exists() else ""
lines = [
    line
    for line in existing.splitlines()
    if not line.lstrip().startswith("openai_base_url")
]
lines.append(f'openai_base_url = "{os.environ["OPENAI_API_BASE"]}"')
config_path.write_text("\n".join(lines).rstrip() + "\n", encoding="utf-8")
EOF_CONFIG
fi

auth_mode="${CODEX_AUTH_MODE:-}"
auth_mode="${auth_mode,,}"
if [[ -z "${auth_mode}" ]]; then
  if [[ -n "${OPENAI_API_KEY:-}" ]]; then
    auth_mode="apikey"
  else
    auth_mode="oauth"
  fi
fi

if [[ "${auth_mode}" == "apikey" && -n "${OPENAI_API_KEY:-}" ]]; then
  python3 - <<'EOF_AUTH'
import json
import os
from pathlib import Path

codex_home = Path(os.environ.get("CODEX_HOME", "/root/.codex"))
codex_home.mkdir(parents=True, exist_ok=True)
(codex_home / "auth.json").write_text(
    json.dumps(
        {
            "auth_mode": "apikey",
            "OPENAI_API_KEY": os.environ["OPENAI_API_KEY"],
        },
        ensure_ascii=False,
        indent=2,
    )
    + "\n",
    encoding="utf-8",
)
EOF_AUTH
  chmod 600 "${CODEX_HOME}/auth.json"
elif [[ "${auth_mode}" == "apikey" ]]; then
  rm -f "${CODEX_HOME}/auth.json"
elif [[ -f "${CODEX_HOME}/auth.json" ]]; then
  chmod 600 "${CODEX_HOME}/auth.json"
fi

ACTIVE_SEED_ROOT="$(/usr/local/bin/prepare-skill-bundle)"
CODEX_SEED_ROOT="${ACTIVE_SEED_ROOT}" /usr/local/bin/sync-seeded-skills
CODEX_SEED_ROOT="${ACTIVE_SEED_ROOT}" /usr/local/bin/verify-skill-bundle

exec "$@"
