#!/usr/bin/env bash
set -Eeuo pipefail

export DEBIAN_FRONTEND=noninteractive

CODEX_HOME="${CODEX_HOME:-/root/.codex}"
CODEX_IMAGE_TOOL_ROOT="${CODEX_IMAGE_TOOL_ROOT:-/opt/codex-image}"
LOCK_FILE="${CODEX_OPTIONAL_TOOLCHAIN_LOCK_FILE:-${CODEX_IMAGE_TOOL_ROOT}/optional_toolchains.lock.json}"
OPTIONAL_ROOT="${CODEX_HOME}/optional-toolchains/ollvm"
STATE_FILE="${OPTIONAL_ROOT}/install-state.json"

FORCE_REBUILD=0
LLVM_MAJOR=""
KOTO_REPO=""
KOTO_REF=""
AKIRA_REPO=""
AKIRA_REF=""

usage() {
  cat <<'EOF_USAGE'
Usage: install-ollvm-toolchain [--force]

Installs the optional OLLVM toolchain bundle into $CODEX_HOME/optional-toolchains/ollvm.
EOF_USAGE
}

log() {
  printf '[install-ollvm] %s\n' "$*" >&2
}

die() {
  printf '[install-ollvm][ERROR] %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

apt_install() {
  apt-get install -y --no-install-recommends "$@"
}

parse_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --force)
        FORCE_REBUILD=1
        shift
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      *)
        usage
        die "unknown arg: $1"
        ;;
    esac
  done
}

load_lock() {
  [[ -f "${LOCK_FILE}" ]] || die "missing lock file: ${LOCK_FILE}"

  local values
  mapfile -t values < <(
    python3 - "${LOCK_FILE}" <<'PY'
import json
import sys
from pathlib import Path

data = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
ollvm = data["toolchains"]["ollvm"]
plugin = ollvm["plugin"]
fork = ollvm["fork"]

print(ollvm["llvm_major"])
print(plugin["repo"])
print(plugin["ref"])
print(fork["repo"])
print(fork["ref"])
PY
  )

  [[ "${#values[@]}" -eq 5 ]] || die "failed to parse ${LOCK_FILE}"
  LLVM_MAJOR="${values[0]}"
  KOTO_REPO="${values[1]}"
  KOTO_REF="${values[2]}"
  AKIRA_REPO="${values[3]}"
  AKIRA_REF="${values[4]}"
}

required_files_exist() {
  [[ -x "${OPTIONAL_ROOT}/llvm-${LLVM_MAJOR}/bin/clang" ]] \
    && [[ -x "${OPTIONAL_ROOT}/llvm-${LLVM_MAJOR}/bin/opt" ]] \
    && [[ -f "${OPTIONAL_ROOT}/koto/Kotoamatsukami.so" ]] \
    && [[ -f "${OPTIONAL_ROOT}/koto/Kotoamatsukami.config" ]] \
    && [[ -f "${OPTIONAL_ROOT}/koto/branch2call_process.py" ]] \
    && [[ -x "${OPTIONAL_ROOT}/akira/bin/clang" ]] \
    && [[ -x "${OPTIONAL_ROOT}/akira/bin/clang++" ]]
}

state_matches_lock() {
  [[ -f "${STATE_FILE}" ]] || return 1

  python3 - "${STATE_FILE}" "${LLVM_MAJOR}" "${KOTO_REPO}" "${KOTO_REF}" "${AKIRA_REPO}" "${AKIRA_REF}" <<'PY'
import json
import sys
from pathlib import Path

state = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
expected = {
    "llvm_major": int(sys.argv[2]),
    "plugin_repo": sys.argv[3],
    "plugin_ref": sys.argv[4],
    "fork_repo": sys.argv[5],
    "fork_ref": sys.argv[6],
}

for key, value in expected.items():
    if state.get(key) != value:
        raise SystemExit(1)
PY
}

ensure_build_prereqs() {
  apt-get update
  apt_install \
    ca-certificates \
    curl \
    git \
    gpg \
    libedit-dev \
    libncurses5-dev \
    libxml2-dev \
    libzstd-dev \
    software-properties-common \
    xz-utils \
    zlib1g-dev
}

ensure_llvm_packages() {
  if [[ -x "/usr/lib/llvm-${LLVM_MAJOR}/bin/clang" && -d "/usr/lib/llvm-${LLVM_MAJOR}/lib/cmake/llvm" ]]; then
    return 0
  fi

  log "installing LLVM ${LLVM_MAJOR} packages into current container"
  curl -fsSL https://apt.llvm.org/llvm.sh -o /tmp/llvm.sh
  chmod +x /tmp/llvm.sh
  /tmp/llvm.sh "${LLVM_MAJOR}" all
  apt-get update
  apt_install \
    "clang-${LLVM_MAJOR}" \
    "lld-${LLVM_MAJOR}" \
    "lldb-${LLVM_MAJOR}" \
    "llvm-${LLVM_MAJOR}" \
    "llvm-${LLVM_MAJOR}-dev" \
    "llvm-${LLVM_MAJOR}-tools"
}

build_koto() {
  local work_root="$1"
  local destination_tmp="$2"
  local repo_root="${work_root}/Kotoamatsukami"

  log "building Kotoamatsukami ${KOTO_REF:0:12}"
  git clone "https://github.com/${KOTO_REPO}.git" "${repo_root}"
  git -C "${repo_root}" checkout "${KOTO_REF}"
  git -C "${repo_root}" submodule update --init --recursive

  python3 - "${repo_root}" "${LLVM_MAJOR}" <<'PY'
from pathlib import Path
import sys

repo_root = Path(sys.argv[1])
llvm_major = sys.argv[2]
cmake_path = repo_root / "CMakeLists.txt"
text = cmake_path.read_text(encoding="utf-8")
replacements = {
    'set(LLVM_DIR "/home/zzzccc/llvm-17/llvm-project/build/lib/cmake/llvm")': f'set(LLVM_DIR "/usr/lib/llvm-{llvm_major}/lib/cmake/llvm")',
    'SOURCE_DIR /home/zzzccc/cxzz/Kotoamatsukami/lib/json': 'SOURCE_DIR ${CMAKE_CURRENT_SOURCE_DIR}/lib/json',
    'include_directories("/home/zzzccc/cxzz/Kotoamatsukami/src/include")': 'include_directories("${CMAKE_CURRENT_SOURCE_DIR}/src/include")',
    'include_directories("/home/zzzccc/cxzz/Kotoamatsukami/lib/json/include")': 'include_directories("${CMAKE_CURRENT_SOURCE_DIR}/lib/json/include")',
    'include_directories("/home/zzzccc/cxzz/Kotoamatsukami/lib/json/single_include/nlohmann")': 'include_directories("${CMAKE_CURRENT_SOURCE_DIR}/lib/json/single_include/nlohmann")',
    'target_link_libraries(Kotoamatsukami nlohmann_json::nlohmann_json)': 'target_link_libraries(Kotoamatsukami PRIVATE nlohmann_json::nlohmann_json)',
}
for old, new in replacements.items():
    text = text.replace(old, new)
cmake_path.write_text(text, encoding="utf-8")
PY

  cmake -S "${repo_root}" -B "${repo_root}/build" -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    "-DLLVM_DIR=/usr/lib/llvm-${LLVM_MAJOR}/lib/cmake/llvm"
  cmake --build "${repo_root}/build" --parallel "$(nproc)"

  install -d "${destination_tmp}"
  cp "${repo_root}/build/Kotoamatsukami.so" "${destination_tmp}/"
  cp "${repo_root}/compiler/branch2call_process.py" "${destination_tmp}/"
  cp "${repo_root}/compiler/Kotoamatsukami.config" "${destination_tmp}/"
  cp "${repo_root}/README.md" "${destination_tmp}/"
}

build_akira() {
  local work_root="$1"
  local destination_tmp="$2"
  local repo_root="${work_root}/akira"
  local build_root="${work_root}/akira-build"
  local clang_real=""
  local clangxx_real=""

  log "building Akira-obfuscator ${AKIRA_REF:0:12}"
  git clone "https://github.com/${AKIRA_REPO}.git" "${repo_root}"
  git -C "${repo_root}" checkout "${AKIRA_REF}"

  python3 - "${repo_root}" <<'PY'
from pathlib import Path
import sys

repo_root = Path(sys.argv[1])
crypto_utils = repo_root / "llvm/lib/Transforms/Obfuscation/CryptoUtils.cpp"
text = crypto_utils.read_text(encoding="utf-8", errors="ignore")
if "#include <fstream>" not in text:
    crypto_utils.write_text('#include <fstream>\n' + text, encoding="utf-8")
PY

  cmake -S "${repo_root}/llvm" -B "${build_root}" -G Ninja \
    -DCMAKE_BUILD_TYPE=Release \
    -DLLVM_ENABLE_ASSERTIONS=OFF \
    -DLLVM_ENABLE_PROJECTS="clang;lld" \
    -DLLVM_TARGETS_TO_BUILD="X86"
  cmake --build "${build_root}" \
    --target clang clang-resource-headers \
    --parallel "$(nproc)"

  clang_real="$(readlink -f "${build_root}/bin/clang")"
  clangxx_real="$(readlink -f "${build_root}/bin/clang++")"

  install -d "${destination_tmp}/bin" "${destination_tmp}/lib"
  cp -a "${clang_real}" "${destination_tmp}/bin/$(basename "${clang_real}")"
  if [[ "${clangxx_real}" != "${clang_real}" ]]; then
    cp -a "${clangxx_real}" "${destination_tmp}/bin/$(basename "${clangxx_real}")"
  fi
  ln -s "$(basename "${clang_real}")" "${destination_tmp}/bin/clang"
  ln -s "$(basename "${clangxx_real}")" "${destination_tmp}/bin/clang++"
  cp -a "${build_root}/lib/clang" "${destination_tmp}/lib/"
}

write_state_file() {
  python3 - "${STATE_FILE}" "${LLVM_MAJOR}" "${KOTO_REPO}" "${KOTO_REF}" "${AKIRA_REPO}" "${AKIRA_REF}" <<'PY'
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

state_path = Path(sys.argv[1])
state = {
    "llvm_major": int(sys.argv[2]),
    "plugin_repo": sys.argv[3],
    "plugin_ref": sys.argv[4],
    "fork_repo": sys.argv[5],
    "fork_ref": sys.argv[6],
    "installed_at": datetime.now(timezone.utc).isoformat(),
}
state_path.write_text(json.dumps(state, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
PY
}

main() {
  parse_args "$@"
  load_lock

  if [[ "${FORCE_REBUILD}" -eq 0 ]] && required_files_exist && state_matches_lock; then
    log "optional OLLVM toolchain already installed at ${OPTIONAL_ROOT}"
    exit 0
  fi

  need_cmd apt-get
  need_cmd cmake
  need_cmd curl
  need_cmd git
  need_cmd ninja
  need_cmd python3

  mkdir -p "${OPTIONAL_ROOT}"
  local work_root
  work_root="$(mktemp -d /tmp/codex-ollvm-build.XXXXXX)"
  trap 'rm -rf "'"${work_root}"'"' EXIT

  ensure_build_prereqs
  ensure_llvm_packages

  local llvm_tmp="${OPTIONAL_ROOT}/llvm-${LLVM_MAJOR}.tmp"
  local koto_tmp="${OPTIONAL_ROOT}/koto.tmp"
  local akira_tmp="${OPTIONAL_ROOT}/akira.tmp"

  rm -rf "${llvm_tmp}" "${koto_tmp}" "${akira_tmp}"
  install -d "${llvm_tmp}" "${koto_tmp}" "${akira_tmp}"

  log "copying LLVM ${LLVM_MAJOR} runtime tree into ${OPTIONAL_ROOT}"
  cp -a "/usr/lib/llvm-${LLVM_MAJOR}/." "${llvm_tmp}/"

  build_koto "${work_root}" "${koto_tmp}"
  build_akira "${work_root}" "${akira_tmp}"

  rm -rf "${OPTIONAL_ROOT}/llvm-${LLVM_MAJOR}" "${OPTIONAL_ROOT}/koto" "${OPTIONAL_ROOT}/akira"
  mv "${llvm_tmp}" "${OPTIONAL_ROOT}/llvm-${LLVM_MAJOR}"
  mv "${koto_tmp}" "${OPTIONAL_ROOT}/koto"
  mv "${akira_tmp}" "${OPTIONAL_ROOT}/akira"
  write_state_file

  log "optional OLLVM toolchain installed into ${OPTIONAL_ROOT}"
}

main "$@"
