#!/usr/bin/env bash
set -Eeuo pipefail

CODEX_HOME="${CODEX_HOME:-/root/.codex}"
OPTIONAL_ROOT="${CODEX_HOME}/optional-toolchains/ollvm"
KOTO_ROOT="${OPTIONAL_ROOT}/koto"

KOTO_SO="${KOTO_ROOT}/Kotoamatsukami.so"
KOTO_CONFIG="${KOTO_ROOT}/Kotoamatsukami.config"
BRANCH2CALL_PROCESS="${KOTO_ROOT}/branch2call_process.py"

fail_missing_install() {
  printf '[optional-toolchain] Koto OLLVM toolchain is not installed. Run: install-ollvm-toolchain\n' >&2
  exit 1
}

LLVM_ROOT="$(find "${OPTIONAL_ROOT}" -maxdepth 1 -mindepth 1 -type d -name 'llvm-*' | sort | head -n 1 || true)"
[[ -n "${LLVM_ROOT}" ]] || fail_missing_install

CLANG_C="${LLVM_ROOT}/bin/clang"
CLANG_CXX="${LLVM_ROOT}/bin/clang++"
OPT="${LLVM_ROOT}/bin/opt"

[[ -x "${CLANG_C}" && -x "${CLANG_CXX}" && -x "${OPT}" ]] || fail_missing_install
[[ -f "${KOTO_SO}" && -f "${KOTO_CONFIG}" && -f "${BRANCH2CALL_PROCESS}" ]] || fail_missing_install

passes=()
clang_args=()
source_file=""
output_file=""
branch2call_enable=0
expect_output_value=0

is_pass() {
  case "$1" in
    split-basic-block|anti-debug|gv-encrypt|bogus-control-flow|add-junk-code|loopen|for-obs|branch2call|branch2call-32|indirect-call|indirect-branch|flatten|substitution)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

for arg in "$@"; do
  if [[ "${expect_output_value}" -eq 1 ]]; then
    output_file="${arg}"
    clang_args+=("-o" "${arg}")
    expect_output_value=0
    continue
  fi

  if is_pass "${arg}"; then
    passes+=("${arg}")
    if [[ "${arg}" == "branch2call" || "${arg}" == "branch2call-32" ]]; then
      branch2call_enable=1
    fi
    continue
  fi

  case "${arg}" in
    -o)
      expect_output_value=1
      ;;
    *.c|*.cc|*.cpp|*.cxx|*.C)
      source_file="${arg}"
      clang_args+=("${arg}")
      ;;
    *)
      clang_args+=("${arg}")
      ;;
  esac
done

compiler="${CLANG_C}"
case "${source_file}" in
  *.cc|*.cpp|*.cxx|*.C)
    compiler="${CLANG_CXX}"
    ;;
esac

if [[ -z "${source_file}" || "${#passes[@]}" -eq 0 ]]; then
  exec "${compiler}" "$@"
fi

if [[ ! -f ./Kotoamatsukami.config ]]; then
  cp -f "${KOTO_CONFIG}" ./Kotoamatsukami.config 2>/dev/null || true
fi

base_name="${source_file%.*}"
ll_file="${base_name}.ll"
obf_ll="${base_name}.obfuscated.ll"
asm_file="${base_name}.s"

filtered_args=()
skip_next=0
for arg in "${clang_args[@]}"; do
  if [[ "${skip_next}" -eq 1 ]]; then
    skip_next=0
    continue
  fi
  if [[ "${arg}" == "${source_file}" ]]; then
    continue
  fi
  if [[ "${arg}" == "-o" ]]; then
    skip_next=1
    continue
  fi
  filtered_args+=("${arg}")
done

"${compiler}" -S -emit-llvm "${filtered_args[@]}" "${source_file}" -o "${ll_file}"
passes_csv="$(IFS=,; printf '%s' "${passes[*]}")"
"${OPT}" --load-pass-plugin="${KOTO_SO}" "${ll_file}" --passes="${passes_csv}" -S -o "${obf_ll}"

if [[ "${branch2call_enable}" -eq 1 ]]; then
  "${compiler}" "${obf_ll}" "${filtered_args[@]}" -Wno-unused-command-line-argument -S -o "${asm_file}"
  python3 "${BRANCH2CALL_PROCESS}" "${asm_file}" "${asm_file}"
  "${compiler}" "${asm_file}" "${filtered_args[@]}" -Wno-unused-command-line-argument -o "${output_file:-${base_name}.out}"
else
  "${compiler}" "${obf_ll}" "${filtered_args[@]}" -Wno-unused-command-line-argument -o "${output_file:-${base_name}.out}"
fi

if [[ "${DEBUG:-0}" != "1" ]]; then
  rm -f "${ll_file}" "${obf_ll}" "${asm_file}"
fi
