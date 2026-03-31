#!/usr/bin/env bash
set -Eeuo pipefail

CODEX_HOME="${CODEX_HOME:-/root/.codex}"
OPTIONAL_ROOT="${CODEX_HOME}/optional-toolchains/ollvm"
COMPILER="${OPTIONAL_ROOT}/akira/bin/clang"

if [[ ! -x "${COMPILER}" ]]; then
  printf '[optional-toolchain] Akira OLLVM compiler is not installed. Run: install-ollvm-toolchain\n' >&2
  exit 1
fi

exec "${COMPILER}" "$@"
