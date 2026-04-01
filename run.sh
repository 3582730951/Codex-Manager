#!/usr/bin/env bash
set -Eeuo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUNTIME_ENV_FILE="${PROJECT_DIR}/.runtime.env"
CODEX_RUNTIME_STATE_DIR="${PROJECT_DIR}/.codex-runtime"
CODEX_RUNTIME_INSTANCES_DIR="${CODEX_RUNTIME_STATE_DIR}/instances"
CODEX_RUNTIME_ACTIVE_INSTANCE_FILE="${CODEX_RUNTIME_STATE_DIR}/active-instance"
CODEXMANAGER_RUNTIME_ENV_FILE="${PROJECT_DIR}/.codexmanager.runtime.env"
CODEXMANAGER_QXCNM_RUNTIME_ENV_FILE="${PROJECT_DIR}/.codexmanager.qxcnm.runtime.env"
CODEXMANAGER_REMOTE_RUNTIME_ENV_FILE="${PROJECT_DIR}/.codexmanager.remote.runtime.env"
CODEX_DOCKERFILE="${PROJECT_DIR}/Dockerfile"
CODEX_COMPOSE_FILE="${PROJECT_DIR}/docker-compose.yml"
CODEX_ENTRYPOINT_FILE="${PROJECT_DIR}/scripts/codex-entrypoint.sh"
CODEX_LOGIN_WRAPPER_FILE="${PROJECT_DIR}/scripts/codex-login-wrapper.sh"
CODEX_OAUTH_PROXY_LOGIN_FILE="${PROJECT_DIR}/scripts/codex-oauth-proxy-login.py"
CODEX_IMAGE_SUPPORT_DIR="${PROJECT_DIR}/scripts/codex-image"
CODEX_TEMPLATE_DIR="${CODEX_IMAGE_SUPPORT_DIR}/templates"
CODEX_DOCKERFILE_TEMPLATE="${CODEX_TEMPLATE_DIR}/Dockerfile.template"
CODEX_COMPOSE_TEMPLATE="${CODEX_TEMPLATE_DIR}/docker-compose.template.yml"
CODEX_ENTRYPOINT_TEMPLATE="${CODEX_TEMPLATE_DIR}/codex-entrypoint.sh"
CODEX_CACHE_DIR="${PROJECT_DIR}/.cache/codex-docker-manager"
CODEX_EMBEDDED_SUPPORT_MARKER="__CODEX_SUPPORT_BUNDLE__"
CODEX_SUPPORT_BUNDLE_STAMP_FILE="${CODEX_IMAGE_SUPPORT_DIR}/.embedded-bundle.sha256"

DEFAULT_IMAGE_NAME="codex:latest"
DEFAULT_CONTAINER_NAME="codex-1"
DEFAULT_NETWORK_NAME="docker_default"
DEFAULT_API_BASE_URL="https://api.openai.com/v1"
DEFAULT_COMPOSE_PROJECT_NAME="codex"
DEFAULT_CODEX_AUTH_MODE="apikey"
DEFAULT_CODEX_AUTH_MODE_FOR_CODEXMANAGER="oauth"
DEFAULT_CODEXMANAGER_ACCESS_MODE="local"
DEFAULT_PORT_3000="3000"
DEFAULT_PORT_5173="5173"
DEFAULT_PORT_8080="8080"
DEFAULT_PORT_1455="1455"
DEFAULT_WORKSPACE_PATH="${PROJECT_DIR}/workspace"

DEFAULT_CODEXMANAGER_REPO_URL="$(git -C "${PROJECT_DIR}" remote get-url origin 2>/dev/null || true)"
DEFAULT_CODEXMANAGER_REPO_URL="${DEFAULT_CODEXMANAGER_REPO_URL:-https://github.com/qxcnm/Codex-Manager.git}"
DEFAULT_CODEXMANAGER_REPO_REF="main"
DEFAULT_CODEXMANAGER_SOURCE_PATH="${PROJECT_DIR}"
DEFAULT_CODEXMANAGER_COMPOSE_PROJECT_NAME="codexmanager"
DEFAULT_CODEXMANAGER_SERVICE_NAME="codexmanager-service"
DEFAULT_CODEXMANAGER_SERVICE_PORT="48760"
DEFAULT_CODEXMANAGER_WEB_PORT="48761"
DEFAULT_CODEXMANAGER_AFFINITY_ROUTING_MODE="enforce"
DEFAULT_CODEXMANAGER_CONTEXT_REPLAY_ENABLED="true"
DEFAULT_CODEXMANAGER_AFFINITY_SOFT_QUOTA_PERCENT="5"
DEFAULT_CODEXMANAGER_REPLAY_MAX_TURNS="12"

DEFAULT_CODEXMANAGER_QXCNM_REPO_URL="https://github.com/qxcnm/Codex-Manager.git"
DEFAULT_CODEXMANAGER_QXCNM_REPO_REF="main"
DEFAULT_CODEXMANAGER_QXCNM_SOURCE_PATH="${CODEX_CACHE_DIR}/codexmanager-qxcnm-source"
DEFAULT_CODEXMANAGER_QXCNM_COMPOSE_PROJECT_NAME="codexmanager-qxcnm"
DEFAULT_CODEXMANAGER_REMOTE_COMPOSE_PROJECT_NAME="codexmanager-remote"
DEFAULT_CODEXMANAGER_REMOTE_SERVER_NAME="_"
DEFAULT_CODEXMANAGER_REMOTE_HTTP_PORT="80"
DEFAULT_CODEXMANAGER_REMOTE_WARP_PROXY_URL="socks5h://127.0.0.1:40000"

CODEX_VOLUME_SUFFIXES=(
  agents_home
  codex_home
  npm_cache
  pnpm_store
  pip_cache
  playwright_cache
  maven_cache
  gradle_cache
  composer_cache
  cargo_cache
  cargo_git
  go_cache
  nuget_cache
)

log() { printf '\n[%s] %s\n' "$(date '+%H:%M:%S')" "$*"; }
warn() { printf '\n[WARN] %s\n' "$*" >&2; }
die() { printf '\n[ERROR] %s\n' "$*" >&2; exit 1; }
need_cmd() { command -v "$1" >/dev/null 2>&1 || die "未找到命令: $1"; }
need_file() { [[ -f "$1" ]] || die "未找到文件: $1"; }

support_bundle_ready() {
  local required_files=(
    "${CODEX_DOCKERFILE_TEMPLATE}"
    "${CODEX_COMPOSE_TEMPLATE}"
    "${CODEX_ENTRYPOINT_TEMPLATE}"
    "${CODEX_LOGIN_WRAPPER_FILE}"
    "${CODEX_OAUTH_PROXY_LOGIN_FILE}"
    "${CODEX_IMAGE_SUPPORT_DIR}/install_runtime_deps.sh"
    "${CODEX_IMAGE_SUPPORT_DIR}/install_ollvm_toolchain.sh"
    "${CODEX_IMAGE_SUPPORT_DIR}/install_skill_bundle.py"
    "${CODEX_IMAGE_SUPPORT_DIR}/prepare_skill_bundle.sh"
    "${CODEX_IMAGE_SUPPORT_DIR}/sync_seeded_skills.sh"
    "${CODEX_IMAGE_SUPPORT_DIR}/verify_skill_bundle.sh"
    "${CODEX_IMAGE_SUPPORT_DIR}/akira-clang.sh"
    "${CODEX_IMAGE_SUPPORT_DIR}/akira-clang++.sh"
    "${CODEX_IMAGE_SUPPORT_DIR}/ollvm-koto-clang.sh"
    "${CODEX_IMAGE_SUPPORT_DIR}/skill_manifest.lock.json"
    "${CODEX_IMAGE_SUPPORT_DIR}/optional_toolchains.lock.json"
  )

  local path
  for path in "${required_files[@]}"; do
    [[ -f "${path}" ]] || return 1
  done
}

embedded_support_bundle_start_line() {
  need_cmd awk

  local start_line
  start_line="$(
    awk -v marker="${CODEX_EMBEDDED_SUPPORT_MARKER}" '
      $0 == marker { print NR + 1; exit }
    ' "${BASH_SOURCE[0]}"
  )"
  [[ -n "${start_line}" ]] || die "run.sh 内未找到内置支撑文件数据。"
  printf '%s' "${start_line}"
}

embedded_support_bundle_sha256() {
  need_cmd sha256sum
  need_cmd tail
  need_cmd tr

  local start_line
  start_line="$(embedded_support_bundle_start_line)"
  tail -n +"${start_line}" "${BASH_SOURCE[0]}" | tr -d '\r' | sha256sum | awk '{print $1}'
}

installed_support_bundle_sha256() {
  [[ -f "${CODEX_SUPPORT_BUNDLE_STAMP_FILE}" ]] || return 1
  tr -d '\r\n' < "${CODEX_SUPPORT_BUNDLE_STAMP_FILE}"
}

refresh_support_bundle_permissions() {
  chmod +x \
    "${CODEX_IMAGE_SUPPORT_DIR}/install_runtime_deps.sh" \
    "${CODEX_IMAGE_SUPPORT_DIR}/install_ollvm_toolchain.sh" \
    "${CODEX_IMAGE_SUPPORT_DIR}/prepare_skill_bundle.sh" \
    "${CODEX_IMAGE_SUPPORT_DIR}/sync_seeded_skills.sh" \
    "${CODEX_IMAGE_SUPPORT_DIR}/verify_skill_bundle.sh" \
    "${CODEX_IMAGE_SUPPORT_DIR}/akira-clang.sh" \
    "${CODEX_IMAGE_SUPPORT_DIR}/akira-clang++.sh" \
    "${CODEX_IMAGE_SUPPORT_DIR}/ollvm-koto-clang.sh" \
    "${CODEX_ENTRYPOINT_TEMPLATE}" \
    "${PROJECT_DIR}/scripts/codex-entrypoint.sh" \
    "${CODEX_LOGIN_WRAPPER_FILE}" \
    "${CODEX_OAUTH_PROXY_LOGIN_FILE}"
}

extract_embedded_support_bundle() {
  need_cmd base64
  need_cmd tail
  need_cmd tar
  need_cmd tr

  local start_line bundle_sha
  start_line="$(embedded_support_bundle_start_line)"
  bundle_sha="$(embedded_support_bundle_sha256)"

  mkdir -p "${PROJECT_DIR}"
  tail -n +"${start_line}" "${BASH_SOURCE[0]}" | tr -d '\r' | base64 -d | tar -xzf - -C "${PROJECT_DIR}"
  printf '%s\n' "${bundle_sha}" > "${CODEX_SUPPORT_BUNDLE_STAMP_FILE}"
  refresh_support_bundle_permissions
}

ensure_support_bundle() {
  local expected_sha current_sha

  if support_bundle_ready; then
    expected_sha="$(embedded_support_bundle_sha256)"
    current_sha="$(installed_support_bundle_sha256 || true)"
    if [[ -n "${current_sha}" && "${current_sha}" == "${expected_sha}" ]]; then
      return 0
    fi

    log "检测到旧版 Codex Docker 支撑文件，正在用当前 run.sh 内置 bundle 覆盖。"
  else
    log "未检测到完整的 Codex Docker 支撑文件，正在从 run.sh 自解压。"
  fi

  extract_embedded_support_bundle
  support_bundle_ready || die "run.sh 自解压完成，但支撑文件仍不完整。"
}

prompt_default() {
  local prompt="$1" default_value="${2:-}" input
  default_value="$(strip_terminal_control_sequences "${default_value}")"
  input="$(read_prompt_input "${prompt} [默认: ${default_value}]: ")"
  if [[ -n "${input}" ]]; then
    printf '%s' "${input}"
  else
    printf '%s' "${default_value}"
  fi
}

prompt_secret_default() {
  local prompt="$1" default_value="${2:-}" input
  # 用户要求 API key 必须明文显示，默认值不再隐藏。
  default_value="$(strip_terminal_control_sequences "${default_value}")"
  input="$(read_prompt_input "${prompt} [默认: ${default_value}]: ")"
  if [[ -n "${input}" ]]; then
    printf '%s' "${input}"
  else
    printf '%s' "${default_value}"
  fi
}

prompt_bool_default() {
  local prompt="$1" default_value="${2:-0}" input
  if [[ "${default_value}" == "1" ]]; then
    input="$(read_prompt_input "${prompt} [Y/n]: ")"
  else
    input="$(read_prompt_input "${prompt} [y/N]: ")"
  fi

  case "${input,,}" in
    "")
      printf '%s' "${default_value}"
      ;;
    y|yes|1|true)
      printf '1'
      ;;
    n|no|0|false)
      printf '0'
      ;;
    *)
      warn "未识别的开关输入，已回退到默认值。"
      printf '%s' "${default_value}"
      ;;
  esac
}

disable_terminal_pointer_reporting() {
  [[ -t 0 ]] || return 0
  [[ -w /dev/tty ]] || return 0

  # 某些终端会遗留 focus/mouse reporting，点击窗口会把 ESC[I / ESC[O 之类的序列直接喂给 read。
  printf '\033[?1000l\033[?1002l\033[?1003l\033[?1004l\033[?1005l\033[?1006l\033[?1015l' > /dev/tty 2>/dev/null || true
}

strip_terminal_control_sequences() {
  local value="${1:-}"
  [[ -n "${value}" ]] || return 0

  value="$(
    printf '%s' "${value}" |
      sed -E $'s/\x1B\\[[0-9;?]*[ -/]*[@-~]//g; s/\x1B[@-_]//g'
  )"
  value="$(printf '%s' "${value}" | tr -d '\000-\010\011\013\014\016-\037\177')"
  printf '%s' "${value}"
}

read_prompt_input() {
  local prompt="$1" input=""
  disable_terminal_pointer_reporting
  IFS= read -r -p "${prompt}" input || true
  strip_terminal_control_sequences "${input}"
}

normalize_api_key_value() {
  local value="${1:-}"
  value="$(strip_terminal_control_sequences "${value}")"
  value="${value//$'\r'/}"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  printf '%s' "${value}"
}

validate_api_key_or_die() {
  local value
  value="$(normalize_api_key_value "${1:-}")"
  [[ -n "${value}" ]] || die "OPENAI_API_KEY 不能为空。"
  [[ "${value}" != *$'\n'* ]] || die "OPENAI_API_KEY 不能包含换行。"
  printf '%s' "${value}"
}

normalize_auth_mode() {
  local value="${1:-}"
  value="$(strip_terminal_control_sequences "${value}")"
  value="${value,,}"
  case "${value}" in
    oauth|apikey) printf '%s' "${value}" ;;
    *) printf '' ;;
  esac
}

get_env_value() {
  local file="$1" key="$2" default_value="${3:-}"
  if [[ -f "${file}" ]]; then
    local line
    line="$(grep -E "^${key}=" "${file}" | head -n 1 || true)"
    if [[ -n "${line}" ]]; then
      printf '%s' "${line#*=}"
      return
    fi
  fi
  printf '%s' "${default_value}"
}

upsert_env_value() {
  local file="$1" key="$2" value="$3"
  mkdir -p "$(dirname "${file}")"
  touch "${file}"
  if grep -qE "^${key}=" "${file}"; then
    sed -i "s|^${key}=.*|${key}=${value}|" "${file}"
  else
    printf '%s=%s\n' "${key}" "${value}" >> "${file}"
  fi
}

normalize_container_name() {
  local value="${1:-}"
  value="$(strip_terminal_control_sequences "${value}")"
  value="${value//$'\r'/}"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  printf '%s' "${value}"
}

validate_container_name_or_die() {
  local value
  value="$(normalize_container_name "${1:-}")"
  [[ -n "${value}" ]] || die "容器名不能为空。"
  [[ "${value}" =~ ^[A-Za-z0-9][A-Za-z0-9_.-]*$ ]] || die "容器名只能包含字母、数字、点、下划线和连字符，且必须以字母或数字开头。"
  printf '%s' "${value}"
}

compose_project_name_for_container() {
  local container_name sanitized
  container_name="$(validate_container_name_or_die "${1:-}")"
  sanitized="${container_name,,}"
  sanitized="$(printf '%s' "${sanitized}" | sed -E 's/[^a-z0-9_-]+/-/g; s/^[^a-z0-9]+//; s/[^a-z0-9]+$//')"
  [[ -n "${sanitized}" ]] || sanitized="${DEFAULT_COMPOSE_PROJECT_NAME}"
  printf '%s' "${sanitized}"
}

normalize_runtime_instance_key() {
  local container_name key
  container_name="$(validate_container_name_or_die "${1:-}")"
  key="${container_name,,}"
  key="$(printf '%s' "${key}" | sed -E 's/[^a-z0-9_.-]+/-/g; s/^[^a-z0-9]+//; s/[^a-z0-9]+$//')"
  [[ -n "${key}" ]] || key="codex-instance"
  printf '%s' "${key}"
}

runtime_env_file_for_container() {
  local key
  key="$(normalize_runtime_instance_key "${1:-}")"
  printf '%s/%s.env' "${CODEX_RUNTIME_INSTANCES_DIR}" "${key}"
}

restore_active_runtime_env_if_needed() {
  local container_name instance_file
  [[ -f "${RUNTIME_ENV_FILE}" ]] && return 0
  [[ -f "${CODEX_RUNTIME_ACTIVE_INSTANCE_FILE}" ]] || return 0

  container_name="$(tr -d '\r\n' < "${CODEX_RUNTIME_ACTIVE_INSTANCE_FILE}")"
  [[ -n "${container_name}" ]] || return 0
  instance_file="$(runtime_env_file_for_container "${container_name}")"
  [[ -f "${instance_file}" ]] || return 0

  mkdir -p "$(dirname "${RUNTIME_ENV_FILE}")"
  cp "${instance_file}" "${RUNTIME_ENV_FILE}"
  chmod 600 "${RUNTIME_ENV_FILE}"
}

save_runtime_env_snapshot() {
  local container_name instance_file
  container_name="$(validate_container_name_or_die "${1:-}")"
  [[ -f "${RUNTIME_ENV_FILE}" ]] || return 0

  instance_file="$(runtime_env_file_for_container "${container_name}")"
  mkdir -p "${CODEX_RUNTIME_INSTANCES_DIR}"
  cp "${RUNTIME_ENV_FILE}" "${instance_file}"
  chmod 600 "${instance_file}"
  mkdir -p "${CODEX_RUNTIME_STATE_DIR}"
  printf '%s\n' "${container_name}" > "${CODEX_RUNTIME_ACTIVE_INSTANCE_FILE}"
}

list_runtime_env_files() {
  local file
  declare -A seen_files=()

  for file in "${RUNTIME_ENV_FILE}" "${CODEX_RUNTIME_INSTANCES_DIR}"/*.env; do
    [[ -f "${file}" ]] || continue
    [[ -z "${seen_files[${file}]+x}" ]] || continue
    seen_files["${file}"]=1
    printf '%s\n' "${file}"
  done
}

list_known_codex_container_names() {
  local file container_name
  declare -A seen_names=()

  while IFS= read -r file; do
    [[ -f "${file}" ]] || continue
    container_name="$(get_env_value "${file}" CONTAINER_NAME "")"
    [[ -n "${container_name}" ]] || continue
    [[ -z "${seen_names[${container_name}]+x}" ]] || continue
    seen_names["${container_name}"]=1
    printf '%s\n' "${container_name}"
  done < <(list_runtime_env_files)

  if command -v docker >/dev/null 2>&1; then
    while IFS= read -r container_name; do
      [[ -n "${container_name}" ]] || continue
      [[ -z "${seen_names[${container_name}]+x}" ]] || continue
      seen_names["${container_name}"]=1
      printf '%s\n' "${container_name}"
    done < <(docker ps -a --format '{{.Names}}' 2>/dev/null || true)
  fi
}

suggest_next_codex_container_name() {
  local container_name max_index=0
  while IFS= read -r container_name; do
    [[ "${container_name}" =~ ^codex-([0-9]+)$ ]] || continue
    (( BASH_REMATCH[1] > max_index )) && max_index="${BASH_REMATCH[1]}"
  done < <(list_known_codex_container_names)

  if (( max_index == 0 )); then
    printf '%s' "${DEFAULT_CONTAINER_NAME}"
  else
    printf 'codex-%s' "$((max_index + 1))"
  fi
}

suggest_next_host_port_default() {
  local key="$1" fallback="$2" file candidate max_port=0
  while IFS= read -r file; do
    candidate="$(get_env_value "${file}" "${key}" "")"
    [[ "${candidate}" =~ ^[0-9]+$ ]] || continue
    (( candidate > max_port )) && max_port="${candidate}"
  done < <(list_runtime_env_files)

  if (( max_port > 0 )); then
    printf '%s' "$((max_port + 1))"
  else
    printf '%s' "${fallback}"
  fi
}

prepare_runtime_env_for_container() {
  local container_name="$1" instance_file compose_project_name
  container_name="$(validate_container_name_or_die "${container_name}")"
  instance_file="$(runtime_env_file_for_container "${container_name}")"

  if [[ -f "${instance_file}" ]]; then
    mkdir -p "$(dirname "${RUNTIME_ENV_FILE}")"
    cp "${instance_file}" "${RUNTIME_ENV_FILE}"
    chmod 600 "${RUNTIME_ENV_FILE}"
    return 0
  fi

  ensure_runtime_env_defaults
  compose_project_name="$(compose_project_name_for_container "${container_name}")"
  upsert_env_value "${RUNTIME_ENV_FILE}" CONTAINER_NAME "${container_name}"
  upsert_env_value "${RUNTIME_ENV_FILE}" COMPOSE_PROJECT_NAME "${compose_project_name}"
  upsert_env_value "${RUNTIME_ENV_FILE}" PORT_3000 "$(suggest_next_host_port_default PORT_3000 "${DEFAULT_PORT_3000}")"
  upsert_env_value "${RUNTIME_ENV_FILE}" PORT_5173 "$(suggest_next_host_port_default PORT_5173 "${DEFAULT_PORT_5173}")"
  upsert_env_value "${RUNTIME_ENV_FILE}" PORT_8080 "$(suggest_next_host_port_default PORT_8080 "${DEFAULT_PORT_8080}")"
  upsert_env_value "${RUNTIME_ENV_FILE}" PORT_1455 "$(suggest_next_host_port_default PORT_1455 "${DEFAULT_PORT_1455}")"
}

normalize_codexmanager_access_mode() {
  local value="${1:-}"
  value="$(strip_terminal_control_sequences "${value}")"
  value="${value,,}"
  case "${value}" in
    local|remote|none) printf '%s' "${value}" ;;
    *) printf '' ;;
  esac
}

codexmanager_access_uses_codexmanager() {
  local value
  value="$(normalize_codexmanager_access_mode "${1:-}")"
  [[ "${value}" == "local" || "${value}" == "remote" ]]
}

default_codex_auth_mode_for_access_mode() {
  local access_mode
  access_mode="$(normalize_codexmanager_access_mode "${1:-}")"
  if codexmanager_access_uses_codexmanager "${access_mode}"; then
    printf '%s' "${DEFAULT_CODEX_AUTH_MODE_FOR_CODEXMANAGER}"
  else
    printf '%s' "${DEFAULT_CODEX_AUTH_MODE}"
  fi
}

normalize_base_url() {
  local value="${1:-}"
  value="$(strip_terminal_control_sequences "${value}")"
  value="${value//$'\r'/}"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  while [[ "${value}" == */ ]]; do
    value="${value%/}"
  done
  printf '%s' "${value}"
}

validate_base_url_or_die() {
  local value
  value="$(normalize_base_url "${1:-}")"
  [[ -n "${value}" ]] || die "基础地址不能为空。"
  [[ "${value}" =~ ^https?://[^[:space:]]+$ ]] || die "基础地址必须是完整的 http/https URL。"
  printf '%s' "${value}"
}

translate_windows_path_to_wsl() {
  local input_path="$1"
  if [[ "${input_path}" =~ ^([A-Za-z]):\\(.*)$ ]]; then
    local drive="${BASH_REMATCH[1],,}"
    local tail_part="${BASH_REMATCH[2]//\\//}"
    printf '/mnt/%s/%s' "${drive}" "${tail_part}"
  else
    printf '%s' "${input_path}"
  fi
}

normalize_path() {
  local value="$1"
  value="$(translate_windows_path_to_wsl "${value}")"
  printf '%s' "${value}"
}

validate_host_port_or_die() {
  local value="$1" label="$2"
  [[ "${value}" =~ ^[0-9]+$ ]] || die "${label} 必须是数字端口。"
  (( value >= 1 && value <= 65535 )) || die "${label} 超出端口范围。"
  printf '%s' "${value}"
}

ensure_distinct_host_ports_or_die() {
  local a="$1" b="$2" c="$3" d="$4"
  [[ "${a}" != "${b}" && "${a}" != "${c}" && "${a}" != "${d}" && "${b}" != "${c}" && "${b}" != "${d}" && "${c}" != "${d}" ]] || die "PORT_3000/PORT_5173/PORT_8080/PORT_1455 不能重复。"
}

ensure_named_network() {
  local network_name="$1"
  if ! docker network inspect "${network_name}" >/dev/null 2>&1; then
    log "未检测到网络 ${network_name}，正在创建。"
    docker network create "${network_name}" >/dev/null
  fi
}

compose_default_network_name() {
  printf '%s_default' "$1"
}

codexmanager_service_api_base() {
  printf 'http://%s:%s/v1' "${DEFAULT_CODEXMANAGER_SERVICE_NAME}" "${DEFAULT_CODEXMANAGER_SERVICE_PORT}"
}

codexmanager_browser_oauth_base() {
  printf 'http://127.0.0.1:%s' "${DEFAULT_CODEXMANAGER_SERVICE_PORT}"
}

codexmanager_container_oauth_base() {
  printf 'http://host.docker.internal:%s' "${DEFAULT_CODEXMANAGER_SERVICE_PORT}"
}

codex_oauth_callback_public_url() {
  local host_port="$1"
  printf 'http://localhost:%s/callback' "${host_port}"
}

codexmanager_remote_api_base() {
  local remote_base
  remote_base="$(validate_base_url_or_die "${1:-}")"
  printf '%s/v1' "${remote_base}"
}

codexmanager_oauth_probe_url() {
  printf 'http://127.0.0.1:%s/oauth/authorize?response_type=code&client_id=codex-cli&redirect_uri=http%%3A%%2F%%2F127.0.0.1%%3A1455%%2Fcallback&state=probe&code_challenge=codexmanagerprobechallengecodexmanager123&code_challenge_method=S256' "${DEFAULT_CODEXMANAGER_SERVICE_PORT}"
}

codexmanager_web_rpc_url() {
  printf 'http://127.0.0.1:%s/api/rpc' "${DEFAULT_CODEXMANAGER_WEB_PORT}"
}

default_codexmanager_remote_base_url() {
  local runtime_value server_name http_port
  runtime_value="$(normalize_base_url "$(get_env_value "${RUNTIME_ENV_FILE}" CODEXMANAGER_REMOTE_BASE_URL "")")"
  if [[ -n "${runtime_value}" ]]; then
    printf '%s' "${runtime_value}"
    return 0
  fi

  [[ -f "${CODEXMANAGER_REMOTE_RUNTIME_ENV_FILE}" ]] || return 0
  server_name="$(get_env_value "${CODEXMANAGER_REMOTE_RUNTIME_ENV_FILE}" CODEXMANAGER_REMOTE_SERVER_NAME "")"
  http_port="$(get_env_value "${CODEXMANAGER_REMOTE_RUNTIME_ENV_FILE}" CODEXMANAGER_REMOTE_HTTP_PORT "${DEFAULT_CODEXMANAGER_REMOTE_HTTP_PORT}")"
  [[ -n "${server_name}" && "${server_name}" != "_" ]] || return 0
  if [[ "${http_port}" == "80" ]]; then
    printf 'http://%s' "${server_name}"
  else
    printf 'http://%s:%s' "${server_name}" "${http_port}"
  fi
}

configured_codexmanager_network_name_from_file() {
  local file="$1" default_project_name="$2" project_name
  project_name="$(get_env_value "${file}" CODEXMANAGER_COMPOSE_PROJECT_NAME "${default_project_name}")"
  get_env_value "${file}" CODEXMANAGER_NETWORK_NAME "$(compose_default_network_name "${project_name}")"
}

list_running_codexmanager_projects() {
  docker ps \
    --filter "label=com.docker.compose.service=${DEFAULT_CODEXMANAGER_SERVICE_NAME}" \
    --format '{{.Label "com.docker.compose.project"}}' 2>/dev/null |
    awk 'NF && !seen[$0]++ { print $0 }'
}

list_active_codexmanager_networks() {
  local file default_project network_name project_name
  declare -A seen_networks=()

  for file in "${CODEXMANAGER_RUNTIME_ENV_FILE}" "${CODEXMANAGER_QXCNM_RUNTIME_ENV_FILE}"; do
    [[ -f "${file}" ]] || continue
    if [[ "${file}" == "${CODEXMANAGER_QXCNM_RUNTIME_ENV_FILE}" ]]; then
      default_project="${DEFAULT_CODEXMANAGER_QXCNM_COMPOSE_PROJECT_NAME}"
    else
      default_project="${DEFAULT_CODEXMANAGER_COMPOSE_PROJECT_NAME}"
    fi
    network_name="$(configured_codexmanager_network_name_from_file "${file}" "${default_project}")"
    [[ -n "${network_name}" ]] || continue
    if docker network inspect "${network_name}" >/dev/null 2>&1 \
      && docker ps --filter "network=${network_name}" --filter "label=com.docker.compose.service=${DEFAULT_CODEXMANAGER_SERVICE_NAME}" -q | grep -q .; then
      [[ -z "${seen_networks[${network_name}]+x}" ]] || continue
      seen_networks["${network_name}"]=1
      printf '%s\n' "${network_name}"
    fi
  done

  while IFS= read -r project_name; do
    [[ -n "${project_name}" ]] || continue
    network_name="$(compose_default_network_name "${project_name}")"
    docker network inspect "${network_name}" >/dev/null 2>&1 || continue
    [[ -z "${seen_networks[${network_name}]+x}" ]] || continue
    seen_networks["${network_name}"]=1
    printf '%s\n' "${network_name}"
  done < <(list_running_codexmanager_projects)
}

active_codexmanager_network_name() {
  local network_name
  while IFS= read -r network_name; do
    [[ -n "${network_name}" ]] || continue
    printf '%s' "${network_name}"
    return 0
  done < <(list_active_codexmanager_networks)
  return 1
}

codexmanager_network_is_selected() {
  local target_network="$1" network_name
  [[ -n "${target_network}" ]] || return 1
  while IFS= read -r network_name; do
    [[ "${target_network}" == "${network_name}" ]] && return 0
  done < <(list_active_codexmanager_networks)
  return 1
}

require_active_local_codexmanager_network_or_die() {
  local target_network="$1"
  [[ -n "${target_network}" ]] || die "local 模式下必须选择一个正在运行的 CodexManager 网络。"
  codexmanager_network_is_selected "${target_network}" || die "local 模式下只能选择正在运行的 CodexManager 网络：${target_network}"
}

suggest_api_base_for_target() {
  local access_mode="$1" network_name="$2" remote_base="$3" fallback="$4"
  access_mode="$(normalize_codexmanager_access_mode "${access_mode}")"
  remote_base="$(normalize_base_url "${remote_base}")"
  case "${access_mode}" in
    local)
      if codexmanager_network_is_selected "${network_name}"; then
        codexmanager_service_api_base
      else
        printf '%s' "${fallback}"
      fi
      ;;
    remote)
      if [[ -n "${remote_base}" ]]; then
        codexmanager_remote_api_base "${remote_base}"
      else
        printf '%s' "${fallback}"
      fi
      ;;
    *)
      printf '%s' "${fallback}"
      ;;
  esac
}

prompt_codexmanager_access_mode() {
  local default_value="$1" selected
  default_value="$(normalize_codexmanager_access_mode "${default_value}")"
  [[ -n "${default_value}" ]] || default_value="${DEFAULT_CODEXMANAGER_ACCESS_MODE}"

  printf '\n请选择 CodexManager 接入方案\n' >&2
  printf '  1) local （默认，接入本地 Docker 里的 CodexManager）\n' >&2
  printf '  2) remote （接入方案10部署出来的远端服务器）\n' >&2
  printf '  3) none （不接入 CodexManager，手动填写 openai_base_url）\n' >&2

  selected="$(read_prompt_input "请选择 [默认: ${default_value}]: ")"
  case "$(normalize_codexmanager_access_mode "${selected}")" in
    local|remote|none)
      normalize_codexmanager_access_mode "${selected}"
      return 0
      ;;
  esac

  case "${selected}" in
    "")
      printf '%s' "${default_value}"
      ;;
    1)
      printf 'local'
      ;;
    2)
      printf 'remote'
      ;;
    3)
      printf 'none'
      ;;
    *)
      warn "未识别的接入方案输入，已回退到默认值 ${default_value}。"
      printf '%s' "${default_value}"
      ;;
  esac
}

prompt_codex_auth_mode() {
  local default_value="$1" access_mode="$2" selected recommended mode_one mode_two
  access_mode="$(normalize_codexmanager_access_mode "${access_mode}")"
  default_value="$(normalize_auth_mode "${default_value}")"
  recommended="$(default_codex_auth_mode_for_access_mode "${access_mode}")"
  [[ -n "${default_value}" ]] || default_value="${recommended}"

  printf '\n请选择 Codex 登录方式\n' >&2
  if codexmanager_access_uses_codexmanager "${access_mode}"; then
    mode_one="oauth"
    mode_two="apikey"
    printf '  1) oauth （推荐，重定向到当前选择的 CodexManager 认证系统）\n' >&2
    printf '  2) apikey （直接预置平台 API Key，不走浏览器 OAuth）\n' >&2
  else
    mode_one="apikey"
    mode_two="oauth"
    printf '  1) apikey （推荐，直连 API Key）\n' >&2
    printf '  2) oauth （不预置 auth.json，后续自行完成官方账号登录）\n' >&2
  fi

  selected="$(read_prompt_input "请选择 [默认: ${default_value}]: ")"
  case "$(normalize_auth_mode "${selected}")" in
    oauth|apikey)
      normalize_auth_mode "${selected}"
      return 0
      ;;
  esac

  case "${selected}" in
    "")
      printf '%s' "${default_value}"
      ;;
    1)
      printf '%s' "${mode_one}"
      ;;
    2)
      printf '%s' "${mode_two}"
      ;;
    *)
      warn "未识别的登录方式输入，已回退到默认值 ${default_value}。"
      printf '%s' "${default_value}"
      ;;
  esac
}

prompt_generic_network_name() {
  local default_value="$1"
  prompt_default "请输入 Codex 要加入的网络名" "${default_value}"
}

prompt_local_codexmanager_network() {
  local default_value="$1" selected next_index network_name
  local options=()

  while IFS= read -r network_name; do
    [[ -n "${network_name}" ]] || continue
    options+=("${network_name}")
  done < <(list_active_codexmanager_networks)

  (( ${#options[@]} > 0 )) || die "未检测到本地 CodexManager 网络，请先执行分支 8 或 9，再回到分支 2 选择 local。"

  if ! codexmanager_network_is_selected "${default_value}"; then
    default_value="${options[0]}"
  fi

  printf '\n请选择本地 CodexManager 网络\n' >&2
  next_index=1
  for network_name in "${options[@]}"; do
    printf '  %s) %s\n' "${next_index}" "${network_name}" >&2
    printf '     - 容器内 API: %s\n' "$(codexmanager_service_api_base)" >&2
    next_index=$((next_index + 1))
  done
  printf '  %s) 自定义网络名\n' "${next_index}" >&2

  selected="$(read_prompt_input "请选择 [默认: ${default_value}]: ")"
  if [[ -z "${selected}" ]]; then
    printf '%s' "${default_value}"
    return 0
  fi

  if [[ "${selected}" =~ ^[0-9]+$ ]]; then
    if (( selected >= 1 && selected <= ${#options[@]} )); then
      printf '%s' "${options[selected-1]}"
      return 0
    fi
    if (( selected == next_index )); then
      prompt_default "请输入自定义网络名" "${default_value}"
      return 0
    fi
  fi

  printf '%s' "${selected}"
}

write_runtime_env() {
  local workspace_path="$1" auth_mode="$2" api_key="$3" image_name="$4" container_name="$5" compose_project_name="$6" network_name="$7" codexmanager_access_mode="$8" codexmanager_remote_base_url="$9" api_base_url="${10}" port_3000="${11}" port_5173="${12}" port_8080="${13}" port_1455="${14}"
  local oauth_browser_base="" oauth_token_base="" oauth_client_id="" oauth_callback_public_url=""
  container_name="$(validate_container_name_or_die "${container_name}")"
  compose_project_name="${compose_project_name:-$(compose_project_name_for_container "${container_name}")}"
  auth_mode="$(normalize_auth_mode "${auth_mode}")"
  codexmanager_access_mode="$(normalize_codexmanager_access_mode "${codexmanager_access_mode}")"
  [[ -n "${codexmanager_access_mode}" ]] || codexmanager_access_mode="${DEFAULT_CODEXMANAGER_ACCESS_MODE}"
  codexmanager_remote_base_url="$(normalize_base_url "${codexmanager_remote_base_url}")"
  [[ -n "${auth_mode}" ]] || auth_mode="$(default_codex_auth_mode_for_access_mode "${codexmanager_access_mode}")"
  if [[ "${auth_mode}" == "oauth" ]]; then
    api_key=""
  elif [[ -n "${api_key}" ]]; then
    api_key="$(validate_api_key_or_die "${api_key}")"
  fi
  if [[ "${auth_mode}" == "oauth" ]]; then
    case "${codexmanager_access_mode}" in
      local)
        oauth_browser_base="$(codexmanager_browser_oauth_base)"
        oauth_token_base="$(codexmanager_container_oauth_base)"
        oauth_client_id="codex-cli"
        oauth_callback_public_url="$(codex_oauth_callback_public_url "${port_1455}")"
        ;;
      remote)
        [[ -n "${codexmanager_remote_base_url}" ]] || die "remote 模式下必须提供远端 CodexManager 基础地址。"
        oauth_browser_base="${codexmanager_remote_base_url}"
        oauth_token_base="${codexmanager_remote_base_url}"
        oauth_client_id="codex-cli"
        oauth_callback_public_url="$(codex_oauth_callback_public_url "${port_1455}")"
        ;;
    esac
  fi
  mkdir -p "$(dirname "${RUNTIME_ENV_FILE}")"
  cat > "${RUNTIME_ENV_FILE}" <<EOF_RUNTIME
IMAGE_NAME=${image_name}
CONTAINER_NAME=${container_name}
NETWORK_NAME=${network_name}
WORKSPACE_PATH=${workspace_path}
COMPOSE_PROJECT_NAME=${compose_project_name}
CODEXMANAGER_ACCESS_MODE=${codexmanager_access_mode}
CODEXMANAGER_REMOTE_BASE_URL=${codexmanager_remote_base_url}
CODEX_AUTH_MODE=${auth_mode}
OPENAI_API_KEY=${api_key}
OPENAI_API_BASE=${api_base_url}
CODEX_API_BASE=${api_base_url}
PORT_3000=${port_3000}
PORT_5173=${port_5173}
PORT_8080=${port_8080}
PORT_1455=${port_1455}
CODEX_OAUTH_BROWSER_ISSUER_BASE_URL=${oauth_browser_base}
CODEX_OAUTH_TOKEN_ISSUER_BASE_URL=${oauth_token_base}
CODEX_OAUTH_CLIENT_ID=${oauth_client_id}
CODEX_OAUTH_CALLBACK_PUBLIC_URL=${oauth_callback_public_url}
CODEX_OAUTH_CALLBACK_BIND_HOST=0.0.0.0
CODEX_OAUTH_CALLBACK_BIND_PORT=1455
COCKPIT_TOOLS_WEB_URL=
COCKPIT_TOOLS_WS_URL=
EOF_RUNTIME
  chmod 600 "${RUNTIME_ENV_FILE}"
}

ensure_runtime_env_defaults() {
  local workspace_path api_key auth_mode image_name container_name compose_project_name network_name codexmanager_access_mode codexmanager_remote_base_url api_base_url port_3000 port_5173 port_8080 port_1455
  restore_active_runtime_env_if_needed
  workspace_path="$(normalize_path "$(get_env_value "${RUNTIME_ENV_FILE}" WORKSPACE_PATH "${DEFAULT_WORKSPACE_PATH}")")"
  image_name="$(get_env_value "${RUNTIME_ENV_FILE}" IMAGE_NAME "${DEFAULT_IMAGE_NAME}")"
  container_name="$(validate_container_name_or_die "$(get_env_value "${RUNTIME_ENV_FILE}" CONTAINER_NAME "${DEFAULT_CONTAINER_NAME}")")"
  compose_project_name="$(get_env_value "${RUNTIME_ENV_FILE}" COMPOSE_PROJECT_NAME "$(compose_project_name_for_container "${container_name}")")"
  network_name="$(get_env_value "${RUNTIME_ENV_FILE}" NETWORK_NAME "${DEFAULT_NETWORK_NAME}")"
  codexmanager_access_mode="$(normalize_codexmanager_access_mode "$(get_env_value "${RUNTIME_ENV_FILE}" CODEXMANAGER_ACCESS_MODE "${DEFAULT_CODEXMANAGER_ACCESS_MODE}")")"
  [[ -n "${codexmanager_access_mode}" ]] || codexmanager_access_mode="${DEFAULT_CODEXMANAGER_ACCESS_MODE}"
  codexmanager_remote_base_url="$(normalize_base_url "$(get_env_value "${RUNTIME_ENV_FILE}" CODEXMANAGER_REMOTE_BASE_URL "$(default_codexmanager_remote_base_url)")")"
  auth_mode="$(normalize_auth_mode "$(get_env_value "${RUNTIME_ENV_FILE}" CODEX_AUTH_MODE "$(default_codex_auth_mode_for_access_mode "${codexmanager_access_mode}")")")"
  [[ -n "${auth_mode}" ]] || auth_mode="$(default_codex_auth_mode_for_access_mode "${codexmanager_access_mode}")"
  if [[ "${auth_mode}" == "oauth" ]]; then
    api_key=""
  else
    api_key="$(normalize_api_key_value "$(get_env_value "${RUNTIME_ENV_FILE}" OPENAI_API_KEY "")")"
  fi
  api_base_url="$(get_env_value "${RUNTIME_ENV_FILE}" OPENAI_API_BASE "$(suggest_api_base_for_target "${codexmanager_access_mode}" "${network_name}" "${codexmanager_remote_base_url}" "${DEFAULT_API_BASE_URL}")")"
  port_3000="$(get_env_value "${RUNTIME_ENV_FILE}" PORT_3000 "${DEFAULT_PORT_3000}")"
  port_5173="$(get_env_value "${RUNTIME_ENV_FILE}" PORT_5173 "${DEFAULT_PORT_5173}")"
  port_8080="$(get_env_value "${RUNTIME_ENV_FILE}" PORT_8080 "${DEFAULT_PORT_8080}")"
  port_1455="$(get_env_value "${RUNTIME_ENV_FILE}" PORT_1455 "${DEFAULT_PORT_1455}")"
  write_runtime_env "${workspace_path}" "${auth_mode}" "${api_key}" "${image_name}" "${container_name}" "${compose_project_name}" "${network_name}" "${codexmanager_access_mode}" "${codexmanager_remote_base_url}" "${api_base_url}" "${port_3000}" "${port_5173}" "${port_8080}" "${port_1455}"
}

collect_build_inputs() {
  ensure_runtime_env_defaults
  local image_name container_name
  image_name="$(prompt_default "请输入镜像名" "$(get_env_value "${RUNTIME_ENV_FILE}" IMAGE_NAME "${DEFAULT_IMAGE_NAME}")")"
  upsert_env_value "${RUNTIME_ENV_FILE}" IMAGE_NAME "${image_name}"
  container_name="$(get_env_value "${RUNTIME_ENV_FILE}" CONTAINER_NAME "")"
  if [[ -n "${container_name}" ]]; then
    save_runtime_env_snapshot "${container_name}"
  fi
}

collect_runtime_inputs() {
  ensure_runtime_env_defaults
  local workspace_path api_key auth_mode image_name container_name compose_project_name network_name codexmanager_access_mode codexmanager_remote_base_url api_base_url port_3000 port_5173 port_8080 port_1455 last_api_base

  container_name="$(validate_container_name_or_die "$(prompt_default "请输入容器名" "$(suggest_next_codex_container_name)")")"
  prepare_runtime_env_for_container "${container_name}"
  ensure_runtime_env_defaults
  compose_project_name="$(compose_project_name_for_container "${container_name}")"

  workspace_path="$(prompt_default "请输入工作区路径（支持 Windows 或 WSL 路径）" "$(get_env_value "${RUNTIME_ENV_FILE}" WORKSPACE_PATH "${DEFAULT_WORKSPACE_PATH}")")"
  workspace_path="$(normalize_path "${workspace_path}")"
  image_name="$(prompt_default "请输入镜像名" "$(get_env_value "${RUNTIME_ENV_FILE}" IMAGE_NAME "${DEFAULT_IMAGE_NAME}")")"
  codexmanager_access_mode="$(prompt_codexmanager_access_mode "$(get_env_value "${RUNTIME_ENV_FILE}" CODEXMANAGER_ACCESS_MODE "${DEFAULT_CODEXMANAGER_ACCESS_MODE}")")"
  case "${codexmanager_access_mode}" in
    local)
      network_name="$(prompt_local_codexmanager_network "$(get_env_value "${RUNTIME_ENV_FILE}" NETWORK_NAME "$(active_codexmanager_network_name || printf '')")")"
      require_active_local_codexmanager_network_or_die "${network_name}"
      codexmanager_remote_base_url=""
      ;;
    remote)
      network_name="$(prompt_generic_network_name "$(get_env_value "${RUNTIME_ENV_FILE}" NETWORK_NAME "${DEFAULT_NETWORK_NAME}")")"
      codexmanager_remote_base_url="$(validate_base_url_or_die "$(prompt_default "请输入远端 CodexManager 基础地址" "$(get_env_value "${RUNTIME_ENV_FILE}" CODEXMANAGER_REMOTE_BASE_URL "$(default_codexmanager_remote_base_url)")")")"
      ;;
    *)
      network_name="$(prompt_generic_network_name "$(get_env_value "${RUNTIME_ENV_FILE}" NETWORK_NAME "${DEFAULT_NETWORK_NAME}")")"
      codexmanager_remote_base_url=""
      ;;
  esac

  auth_mode="$(prompt_codex_auth_mode "$(get_env_value "${RUNTIME_ENV_FILE}" CODEX_AUTH_MODE "$(default_codex_auth_mode_for_access_mode "${codexmanager_access_mode}")")" "${codexmanager_access_mode}")"
  if [[ "${auth_mode}" == "apikey" ]]; then
    api_key="$(validate_api_key_or_die "$(prompt_secret_default "请输入 OPENAI_API_KEY" "$(normalize_api_key_value "$(get_env_value "${RUNTIME_ENV_FILE}" OPENAI_API_KEY "")")")")"
  else
    api_key=""
  fi
  last_api_base="$(get_env_value "${RUNTIME_ENV_FILE}" OPENAI_API_BASE "${DEFAULT_API_BASE_URL}")"
  api_base_url="$(suggest_api_base_for_target "${codexmanager_access_mode}" "${network_name}" "${codexmanager_remote_base_url}" "${last_api_base}")"
  case "${codexmanager_access_mode}" in
    local)
      log "已自动将 Codex API 地址切换到本地 CodexManager：${api_base_url}"
      ;;
    remote)
      log "已自动将 Codex API 地址切换到远端 CodexManager：${api_base_url}"
      ;;
    *)
      api_base_url="$(validate_base_url_or_die "$(prompt_default "请输入 openai_base_url" "${api_base_url}")")"
      ;;
  esac
  port_3000="$(validate_host_port_or_die "$(prompt_default "请输入宿主机 3000 对外端口" "$(get_env_value "${RUNTIME_ENV_FILE}" PORT_3000 "${DEFAULT_PORT_3000}")")" "PORT_3000")"
  port_5173="$(validate_host_port_or_die "$(prompt_default "请输入宿主机 5173 对外端口" "$(get_env_value "${RUNTIME_ENV_FILE}" PORT_5173 "${DEFAULT_PORT_5173}")")" "PORT_5173")"
  port_8080="$(validate_host_port_or_die "$(prompt_default "请输入宿主机 8080 对外端口" "$(get_env_value "${RUNTIME_ENV_FILE}" PORT_8080 "${DEFAULT_PORT_8080}")")" "PORT_8080")"
  port_1455="$(validate_host_port_or_die "$(prompt_default "请输入宿主机 OAuth 回调对外端口（映射到容器 1455）" "$(get_env_value "${RUNTIME_ENV_FILE}" PORT_1455 "${DEFAULT_PORT_1455}")")" "PORT_1455")"
  ensure_distinct_host_ports_or_die "${port_3000}" "${port_5173}" "${port_8080}" "${port_1455}"

  mkdir -p "${workspace_path}"
  write_runtime_env "${workspace_path}" "${auth_mode}" "${api_key}" "${image_name}" "${container_name}" "${compose_project_name}" "${network_name}" "${codexmanager_access_mode}" "${codexmanager_remote_base_url}" "${api_base_url}" "${port_3000}" "${port_5173}" "${port_8080}" "${port_1455}"
  save_runtime_env_snapshot "${container_name}"
  log "已写入 ${RUNTIME_ENV_FILE}"
  log "当前实例: ${container_name} （compose project: ${compose_project_name}）"
  log "当前 Codex 登录模式: ${auth_mode}"
  if [[ "${auth_mode}" == "apikey" ]]; then
    log "当前 OPENAI_API_KEY: ${api_key}"
  else
    log "OAuth 模式下不会预置 auth.json，后续请在容器内完成账号登录。"
    case "${codexmanager_access_mode}" in
      local)
        log "当前容器内 codex login 将改走本地 CodexManager OAuth 兼容层。"
        log "浏览器 OAuth 入口: $(codexmanager_browser_oauth_base)/oauth/authorize"
        log "容器内 token issuer: $(codexmanager_container_oauth_base)"
        log "浏览器回调地址: $(codex_oauth_callback_public_url "${port_1455}")"
        ;;
      remote)
        log "当前容器内 codex login 将改走远端 CodexManager OAuth 兼容层。"
        log "浏览器 OAuth 入口: ${codexmanager_remote_base_url}/oauth/authorize"
        log "容器内 token issuer: ${codexmanager_remote_base_url}"
        log "浏览器回调地址: $(codex_oauth_callback_public_url "${port_1455}")"
        ;;
      *)
        log "当前未接入 CodexManager，容器内 codex login 将保持官方 OAuth 行为。"
        ;;
    esac
  fi
  if codexmanager_access_uses_codexmanager "${codexmanager_access_mode}" && [[ "${auth_mode}" == "apikey" ]]; then
    warn "当前已接入 CodexManager，但仍选择了 apikey 模式；这不会走实例 OAuth 识别链路。"
  fi
}

generate_files() {
  ensure_support_bundle
  mkdir -p "${PROJECT_DIR}/scripts" "${PROJECT_DIR}/workspace"
  need_file "${CODEX_DOCKERFILE_TEMPLATE}"
  need_file "${CODEX_COMPOSE_TEMPLATE}"
  need_file "${CODEX_ENTRYPOINT_TEMPLATE}"

  cp "${CODEX_DOCKERFILE_TEMPLATE}" "${CODEX_DOCKERFILE}"
  cp "${CODEX_COMPOSE_TEMPLATE}" "${CODEX_COMPOSE_FILE}"
  cp "${CODEX_ENTRYPOINT_TEMPLATE}" "${CODEX_ENTRYPOINT_FILE}"
  chmod +x "${CODEX_ENTRYPOINT_FILE}"

  ensure_runtime_env_defaults
  log "已生成 ${CODEX_DOCKERFILE}、${CODEX_COMPOSE_FILE} 和 ${CODEX_ENTRYPOINT_FILE}"
}

codex_compose_project_name() {
  local container_name
  restore_active_runtime_env_if_needed
  container_name="$(get_env_value "${RUNTIME_ENV_FILE}" CONTAINER_NAME "${DEFAULT_CONTAINER_NAME}")"
  get_env_value "${RUNTIME_ENV_FILE}" COMPOSE_PROJECT_NAME "$(compose_project_name_for_container "${container_name}")"
}

ensure_runtime_env_exists() {
  restore_active_runtime_env_if_needed
  [[ -f "${RUNTIME_ENV_FILE}" ]] || die "未找到 ${RUNTIME_ENV_FILE}，请先执行菜单 1 或 2。"
}

build_image() {
  need_cmd docker
  generate_files
  ensure_runtime_env_exists
  DOCKER_BUILDKIT=1 docker compose -p "$(codex_compose_project_name)" --env-file "${RUNTIME_ENV_FILE}" build
  log "Codex 基础镜像构建完成（默认包含预置 skills，不再默认内置 OLLVM）。"
}

start_container() {
  need_cmd docker
  generate_files
  ensure_runtime_env_exists
  ensure_named_network "$(get_env_value "${RUNTIME_ENV_FILE}" NETWORK_NAME "${DEFAULT_NETWORK_NAME}")"
  docker compose -p "$(codex_compose_project_name)" --env-file "${RUNTIME_ENV_FILE}" up -d --force-recreate --remove-orphans
  log "Codex 容器已启动；首次启动会自动把镜像内预置 skills 同步到持久化卷。OLLVM 改为手动安装。"
}

enter_container() {
  need_cmd docker
  ensure_runtime_env_exists
  local container_name
  container_name="$(get_env_value "${RUNTIME_ENV_FILE}" CONTAINER_NAME "${DEFAULT_CONTAINER_NAME}")"
  docker inspect "${container_name}" >/dev/null 2>&1 || die "容器 ${container_name} 不存在，请先执行菜单 2。"
  docker exec -it "${container_name}" bash
}

stop_remove_container() {
  need_cmd docker
  ensure_runtime_env_exists
  local container_name
  container_name="$(get_env_value "${RUNTIME_ENV_FILE}" CONTAINER_NAME "${DEFAULT_CONTAINER_NAME}")"
  if docker inspect "${container_name}" >/dev/null 2>&1; then
    docker rm -f "${container_name}" >/dev/null
    log "已删除容器 ${container_name}"
  else
    warn "容器 ${container_name} 不存在。"
  fi
}

install_optional_ollvm_in_container() {
  need_cmd docker
  ensure_runtime_env_exists

  local container_name running
  container_name="$(get_env_value "${RUNTIME_ENV_FILE}" CONTAINER_NAME "${DEFAULT_CONTAINER_NAME}")"
  docker inspect "${container_name}" >/dev/null 2>&1 || die "容器 ${container_name} 不存在，请先执行菜单 2。"
  running="$(docker inspect -f '{{.State.Running}}' "${container_name}")"
  [[ "${running}" == "true" ]] || die "容器 ${container_name} 未运行，请先启动容器。"

  docker exec -it "${container_name}" bash -lc 'install-ollvm-toolchain'
  log "已在容器 ${container_name} 中完成可选 OLLVM 工具链安装。"
}

remove_image_interactive() {
  need_cmd docker
  ensure_runtime_env_defaults
  local compose_project_name image_name network_name suffix volume_name codexmanager_access_mode
  compose_project_name="$(codex_compose_project_name)"
  image_name="$(get_env_value "${RUNTIME_ENV_FILE}" IMAGE_NAME "${DEFAULT_IMAGE_NAME}")"
  network_name="$(get_env_value "${RUNTIME_ENV_FILE}" NETWORK_NAME "${DEFAULT_NETWORK_NAME}")"
  codexmanager_access_mode="$(normalize_codexmanager_access_mode "$(get_env_value "${RUNTIME_ENV_FILE}" CODEXMANAGER_ACCESS_MODE "${DEFAULT_CODEXMANAGER_ACCESS_MODE}")")"

  if [[ -f "${CODEX_COMPOSE_FILE}" ]]; then
    docker compose -p "${compose_project_name}" --env-file "${RUNTIME_ENV_FILE}" down -v --remove-orphans >/dev/null 2>&1 || true
  fi
  docker image inspect "${image_name}" >/dev/null 2>&1 && docker rmi -f "${image_name}" >/dev/null 2>&1 || true

  for suffix in "${CODEX_VOLUME_SUFFIXES[@]}"; do
    volume_name="${compose_project_name}_${suffix}"
    docker volume inspect "${volume_name}" >/dev/null 2>&1 && docker volume rm -f "${volume_name}" >/dev/null 2>&1 || true
  done

  if [[ "${codexmanager_access_mode}" == "local" ]] && codexmanager_network_is_selected "${network_name}"; then
    log "检测到 ${network_name} 是正在使用的 CodexManager 共享网络，已跳过网络删除。"
  else
    docker network inspect "${network_name}" >/dev/null 2>&1 && docker network rm "${network_name}" >/dev/null 2>&1 || true
  fi
  log "已清理 Codex 镜像、容器、关联网络和卷。"
}

write_codexmanager_runtime_env() {
  local file="$1" repo_url="$2" repo_ref="$3" source_path="$4" compose_project_name="$5"
  local network_name
  network_name="$(compose_default_network_name "${compose_project_name}")"
  cat > "${file}" <<EOF_CODEXMANAGER
CODEXMANAGER_REPO_URL=${repo_url}
CODEXMANAGER_REPO_REF=${repo_ref}
CODEXMANAGER_SOURCE_PATH=${source_path}
CODEXMANAGER_COMPOSE_PROJECT_NAME=${compose_project_name}
CODEXMANAGER_NETWORK_NAME=${network_name}
EOF_CODEXMANAGER
  chmod 600 "${file}"
}

write_codexmanager_remote_runtime_env() {
  local file="$1" repo_url="$2" repo_ref="$3" source_path="$4" compose_project_name="$5" server_name="$6" http_port="$7" warp_enabled="$8" warp_proxy_url="$9"
  local network_name nginx_conf_path
  network_name="$(compose_default_network_name "${compose_project_name}")"
  nginx_conf_path="${source_path}/docker/nginx/codexmanager.remote.generated.conf"
  cat > "${file}" <<EOF_CODEXMANAGER_REMOTE
CODEXMANAGER_REPO_URL=${repo_url}
CODEXMANAGER_REPO_REF=${repo_ref}
CODEXMANAGER_SOURCE_PATH=${source_path}
CODEXMANAGER_COMPOSE_PROJECT_NAME=${compose_project_name}
CODEXMANAGER_NETWORK_NAME=${network_name}
CODEXMANAGER_REMOTE_SERVER_NAME=${server_name}
CODEXMANAGER_REMOTE_HTTP_PORT=${http_port}
CODEXMANAGER_REMOTE_WARP_ENABLED=${warp_enabled}
CODEXMANAGER_GATEWAY_ACCOUNT_PROXY_URL=${warp_proxy_url}
CODEXMANAGER_REMOTE_NGINX_CONF_PATH=${nginx_conf_path}
EOF_CODEXMANAGER_REMOTE
  chmod 600 "${file}"
}

collect_codexmanager_remote_runtime_inputs() {
  local file="$1" default_repo_url="$2" default_repo_ref="$3" default_source_path="$4" default_project_name="$5" target_label="$6"
  local repo_url repo_ref source_path compose_project_name server_name http_port warp_enabled warp_proxy_url

  repo_url="$(prompt_default "请输入${target_label}仓库地址" "$(get_env_value "${file}" CODEXMANAGER_REPO_URL "${default_repo_url}")")"
  repo_ref="$(prompt_default "请输入${target_label}分支/标签" "$(get_env_value "${file}" CODEXMANAGER_REPO_REF "${default_repo_ref}")")"
  source_path="$(prompt_default "请输入${target_label}源码路径（如果输入未被独占的目录可自动拉取）" "$(get_env_value "${file}" CODEXMANAGER_SOURCE_PATH "${default_source_path}")")"
  source_path="$(normalize_path "${source_path}")"
  compose_project_name="$(prompt_default "请输入${target_label} compose project name" "$(get_env_value "${file}" CODEXMANAGER_COMPOSE_PROJECT_NAME "${default_project_name}")")"
  server_name="$(prompt_default "请输入远程 Nginx server_name（域名或 _）" "$(get_env_value "${file}" CODEXMANAGER_REMOTE_SERVER_NAME "${DEFAULT_CODEXMANAGER_REMOTE_SERVER_NAME}")")"
  http_port="$(validate_host_port_or_die "$(prompt_default "请输入远程 Nginx 对外 HTTP 端口" "$(get_env_value "${file}" CODEXMANAGER_REMOTE_HTTP_PORT "${DEFAULT_CODEXMANAGER_REMOTE_HTTP_PORT}")")" "CODEXMANAGER_REMOTE_HTTP_PORT")"
  warp_enabled="$(prompt_bool_default "是否启用 WARP 模式（通过专用代理出站）" "$(get_env_value "${file}" CODEXMANAGER_REMOTE_WARP_ENABLED "0")")"
  if [[ "${warp_enabled}" == "1" ]]; then
    warp_proxy_url="$(prompt_default "请输入 WARP/代理 URL（支持 socks5/http）" "$(get_env_value "${file}" CODEXMANAGER_GATEWAY_ACCOUNT_PROXY_URL "${DEFAULT_CODEXMANAGER_REMOTE_WARP_PROXY_URL}")")"
    [[ -n "${warp_proxy_url}" ]] || die "启用 WARP 模式时，代理 URL 不能为空。"
  else
    warp_proxy_url=""
  fi

  write_codexmanager_remote_runtime_env "${file}" "${repo_url}" "${repo_ref}" "${source_path}" "${compose_project_name}" "${server_name}" "${http_port}" "${warp_enabled}" "${warp_proxy_url}"
  log "已写入 ${file}"
}

collect_codexmanager_runtime_inputs() {
  local file="$1" default_repo_url="$2" default_repo_ref="$3" default_source_path="$4" default_project_name="$5" target_label="$6"
  local repo_url repo_ref source_path compose_project_name

  repo_url="$(prompt_default "请输入${target_label}仓库地址" "$(get_env_value "${file}" CODEXMANAGER_REPO_URL "${default_repo_url}")")"
  repo_ref="$(prompt_default "请输入${target_label}分支/标签" "$(get_env_value "${file}" CODEXMANAGER_REPO_REF "${default_repo_ref}")")"
  source_path="$(prompt_default "请输入${target_label}源码路径（如果输入未被独占的目录可自动拉取）" "$(get_env_value "${file}" CODEXMANAGER_SOURCE_PATH "${default_source_path}")")"
  source_path="$(normalize_path "${source_path}")"
  compose_project_name="$(prompt_default "请输入${target_label} compose project name" "$(get_env_value "${file}" CODEXMANAGER_COMPOSE_PROJECT_NAME "${default_project_name}")")"

  write_codexmanager_runtime_env "${file}" "${repo_url}" "${repo_ref}" "${source_path}" "${compose_project_name}"
  log "已写入 ${file}"
}

resolve_codexmanager_compose_file() {
  local source_path="$1"
  if [[ -f "${source_path}/docker/docker-compose.yml" ]]; then
    printf '%s' "${source_path}/docker/docker-compose.yml"
    return 0
  fi
  if [[ -f "${source_path}/crates/docker/docker-compose.yml" ]]; then
    printf '%s' "${source_path}/crates/docker/docker-compose.yml"
    return 0
  fi
  return 1
}

resolve_codexmanager_remote_compose_file() {
  local source_path="$1"
  if [[ -f "${source_path}/docker/docker-compose.remote.yml" ]]; then
    printf '%s' "${source_path}/docker/docker-compose.remote.yml"
    return 0
  fi
  return 1
}

render_codexmanager_remote_nginx_conf() {
  local file="$1" source_path="$2"
  local template_path output_path server_name escaped_server_name

  template_path="${source_path}/docker/nginx/codexmanager.remote.conf.template"
  output_path="$(get_env_value "${file}" CODEXMANAGER_REMOTE_NGINX_CONF_PATH "${source_path}/docker/nginx/codexmanager.remote.generated.conf")"
  server_name="$(get_env_value "${file}" CODEXMANAGER_REMOTE_SERVER_NAME "${DEFAULT_CODEXMANAGER_REMOTE_SERVER_NAME}")"

  [[ -f "${template_path}" ]] || die "未找到远程 nginx 模板：${template_path}"
  mkdir -p "$(dirname "${output_path}")"
  escaped_server_name="$(printf '%s' "${server_name}" | sed 's/[\/&]/\\&/g')"
  sed "s/__SERVER_NAME__/${escaped_server_name}/g" "${template_path}" > "${output_path}"
  chmod 644 "${output_path}"
}

prepare_codexmanager_source() {
  local file="$1"
  local repo_url repo_ref source_path current_origin

  repo_url="$(get_env_value "${file}" CODEXMANAGER_REPO_URL "${DEFAULT_CODEXMANAGER_REPO_URL}")"
  repo_ref="$(get_env_value "${file}" CODEXMANAGER_REPO_REF "${DEFAULT_CODEXMANAGER_REPO_REF}")"
  source_path="$(normalize_path "$(get_env_value "${file}" CODEXMANAGER_SOURCE_PATH "${PROJECT_DIR}")")"

  if [[ "${source_path}" == "${PROJECT_DIR}" ]]; then
    log "使用当前工作区源码：${PROJECT_DIR}"
    return 0
  fi

  mkdir -p "$(dirname "${source_path}")"
  if [[ -d "${source_path}/.git" ]]; then
    current_origin="$(git -C "${source_path}" remote get-url origin 2>/dev/null || true)"
    if [[ "${current_origin}" != "${repo_url}" ]]; then
      rm -rf "${source_path}"
    fi
  elif [[ -e "${source_path}" ]]; then
    die "源码目录已存在但不是 Git 仓库：${source_path}"
  fi

  if [[ ! -d "${source_path}/.git" ]]; then
    log "正在拉取源码：${repo_url} (${repo_ref})"
    git clone "${repo_url}" "${source_path}"
  else
    log "正在同步源码：${repo_url} (${repo_ref})"
  fi

  git -C "${source_path}" remote set-url origin "${repo_url}"
  git -C "${source_path}" fetch --prune --tags origin
  git -C "${source_path}" reset --hard >/dev/null
  git -C "${source_path}" clean -fdx >/dev/null
  if git -C "${source_path}" show-ref --verify --quiet "refs/remotes/origin/${repo_ref}"; then
    git -C "${source_path}" checkout -B "${repo_ref}" "origin/${repo_ref}" >/dev/null
  else
    git -C "${source_path}" checkout --detach "${repo_ref}" >/dev/null
  fi
}

http_status_is_ready() {
  case "$1" in
    200|204|301|302|303|307|308|401|403|404) return 0 ;;
    *) return 1 ;;
  esac
}

wait_for_http_ready() {
  local label="$1" url="$2" max_attempts="${3:-45}" attempt status_code
  for ((attempt=1; attempt<=max_attempts; attempt+=1)); do
    status_code="$(curl -L -s -o /dev/null -w '%{http_code}' "${url}" || true)"
    if http_status_is_ready "${status_code}"; then
      log "${label} 已就绪（HTTP ${status_code}）：${url}"
      return 0
    fi
    sleep 2
  done
  warn "${label} 在等待窗口内未确认就绪：${url}"
  return 1
}

wait_for_codexmanager_stack_ready() {
  wait_for_http_ready "CodexManager API" "http://127.0.0.1:${DEFAULT_CODEXMANAGER_SERVICE_PORT}/health" 45 || return 1
  wait_for_http_ready "CodexManager OAuth" "$(codexmanager_oauth_probe_url)" 45 || return 1
  wait_for_http_ready "CodexManager Web" "http://127.0.0.1:${DEFAULT_CODEXMANAGER_WEB_PORT}/" 45 || return 1
}

wait_for_codexmanager_remote_stack_ready() {
  local http_port="$1"
  wait_for_http_ready "Remote Nginx Web" "http://127.0.0.1:${http_port}/" 45 || return 1
  wait_for_http_ready "Remote Nginx OAuth" "http://127.0.0.1:${http_port}/oauth/authorize?response_type=code&client_id=codex-cli&redirect_uri=http%3A%2F%2F127.0.0.1%3A1455%2Fcallback&state=probe&code_challenge=codexmanagerprobechallengecodexmanager123&code_challenge_method=S256" 45 || return 1
  wait_for_http_ready "Remote Nginx Gateway" "http://127.0.0.1:${http_port}/health" 45 || return 1
}

host_port_is_published_by_other_container() {
  local port="$1" expected_project="$2" ports_field published_project
  while IFS='|' read -r ports_field published_project; do
    [[ -n "${ports_field}" ]] || continue
    [[ "${ports_field}" == *":${port}->"* ]] || continue
    [[ -n "${expected_project}" && "${published_project}" == "${expected_project}" ]] && continue
    return 0
  done < <(docker ps --format '{{.Ports}}|{{.Label "com.docker.compose.project"}}')
  return 1
}

ensure_local_codexmanager_ports_available_or_owned_by_project() {
  local compose_project_name="$1" branch_label="${2:-CodexManager 部署}"
  if host_port_is_published_by_other_container "${DEFAULT_CODEXMANAGER_SERVICE_PORT}" "${compose_project_name}"; then
    die "宿主机端口 ${DEFAULT_CODEXMANAGER_SERVICE_PORT} 已被其他容器占用，${branch_label} 无法继续部署。"
  fi
  if host_port_is_published_by_other_container "${DEFAULT_CODEXMANAGER_WEB_PORT}" "${compose_project_name}"; then
    die "宿主机端口 ${DEFAULT_CODEXMANAGER_WEB_PORT} 已被其他容器占用，${branch_label} 无法继续部署。"
  fi
}

verify_codex_runtime_against_codexmanager_or_die() {
  local codexmanager_access_mode="$1" network_name="$2" auth_mode="$3" codexmanager_remote_base_url="$4" container_name remote_health_url
  codexmanager_access_mode="$(normalize_codexmanager_access_mode "${codexmanager_access_mode}")"
  codexmanager_remote_base_url="$(normalize_base_url "${codexmanager_remote_base_url}")"
  container_name="$(get_env_value "${RUNTIME_ENV_FILE}" CONTAINER_NAME "${DEFAULT_CONTAINER_NAME}")"

  if [[ "${auth_mode}" == "oauth" ]]; then
    if docker exec "${container_name}" bash -lc 'test -f "${CODEX_HOME:-/root/.codex}/auth.json"' >/dev/null 2>&1; then
      log "OAuth 模式检测到容器内已有 auth.json，将保留现有登录态。"
    else
      log "已确认 OAuth 模式未向容器预置 auth.json。"
    fi
  fi

  case "${codexmanager_access_mode}" in
    local)
      if ! docker exec "${container_name}" bash -lc "curl -fsS http://${DEFAULT_CODEXMANAGER_SERVICE_NAME}:${DEFAULT_CODEXMANAGER_SERVICE_PORT}/health >/dev/null" >/dev/null 2>&1; then
        docker rm -f "${container_name}" >/dev/null 2>&1 || true
        die "当前容器无法通过网络 ${network_name} 访问本地 CodexManager 服务，请先执行分支 8/9 或检查所选网络。"
      fi
      log "已确认当前 Codex 容器可访问本地 CodexManager：$(codexmanager_service_api_base)"
      ;;
    remote)
      remote_health_url="$(printf '%q' "${codexmanager_remote_base_url}/health")"
      if ! docker exec "${container_name}" bash -lc "curl -fsS ${remote_health_url} >/dev/null" >/dev/null 2>&1; then
        docker rm -f "${container_name}" >/dev/null 2>&1 || true
        die "当前容器无法访问远端 CodexManager：${codexmanager_remote_base_url}/health，请检查公网地址、TLS 或防火墙。"
      fi
      log "已确认当前 Codex 容器可访问远端 CodexManager：${codexmanager_remote_base_url}"
      ;;
  esac

  if [[ "${auth_mode}" == "apikey" && "${codexmanager_access_mode}" == "remote" ]]; then
    log "远端模式下 API Key 登录会直接通过 ${codexmanager_remote_base_url}/v1 访问平台。"
  fi
}

codexmanager_rpc_call() {
  local payload="$1"
  curl -fsS \
    -H 'Content-Type: application/json' \
    -d "${payload}" \
    "$(codexmanager_web_rpc_url)"
}

ensure_codexmanager_affinity_defaults() {
  local current_response set_response

  current_response="$(
    codexmanager_rpc_call '{"jsonrpc":"2.0","id":1,"method":"gateway/affinity/get","params":{}}' 2>/dev/null || true
  )"

  if [[ -z "${current_response}" ]]; then
    warn "未能读取当前 affinity 配置，跳过默认启用。"
    return 1
  fi

  if grep -q "\"affinityRoutingMode\":\"${DEFAULT_CODEXMANAGER_AFFINITY_ROUTING_MODE}\"" <<< "${current_response}"; then
    log "检测到 affinity 已为 ${DEFAULT_CODEXMANAGER_AFFINITY_ROUTING_MODE}，无需修改。"
    return 0
  fi

  if ! grep -q '"affinityRoutingMode":"off"' <<< "${current_response}"; then
    log "检测到 affinity 已被显式配置为非 off，保持现状。"
    return 0
  fi

  set_response="$(
    codexmanager_rpc_call "$(cat <<EOF_RPC
{"jsonrpc":"2.0","id":2,"method":"gateway/affinity/set","params":{"affinityRoutingMode":"${DEFAULT_CODEXMANAGER_AFFINITY_ROUTING_MODE}","contextReplayEnabled":${DEFAULT_CODEXMANAGER_CONTEXT_REPLAY_ENABLED},"affinitySoftQuotaPercent":${DEFAULT_CODEXMANAGER_AFFINITY_SOFT_QUOTA_PERCENT},"replayMaxTurns":${DEFAULT_CODEXMANAGER_REPLAY_MAX_TURNS}}}
EOF_RPC
)" 2>/dev/null || true
  )"

  if grep -q "\"affinityRoutingMode\":\"${DEFAULT_CODEXMANAGER_AFFINITY_ROUTING_MODE}\"" <<< "${set_response}"; then
    log "已将 affinity 默认切换为 ${DEFAULT_CODEXMANAGER_AFFINITY_ROUTING_MODE}。"
    return 0
  fi

  warn "已尝试默认启用 affinity，但未确认成功；请稍后在设置页或日志中复核。"
  return 1
}

follow_codexmanager_compose_logs() {
  local source_path="$1" compose_project_name="$2" compose_file="$3" branch_label="$4" env_file="${5:-}"
  (
    trap 'exit 0' INT TERM
    cd "${source_path}"
    if [[ -n "${env_file}" && -f "${env_file}" ]]; then
      COMPOSE_PROJECT_NAME="${compose_project_name}" docker compose --env-file "${env_file}" -f "${compose_file}" logs --follow --tail=100
    else
      COMPOSE_PROJECT_NAME="${compose_project_name}" docker compose -f "${compose_file}" logs --follow --tail=100
    fi
  ) || true
  log "已退出${branch_label}日志视图；后台服务仍在运行。需要停服时，请执行 docker compose down。"
}

show_codexmanager_access_info_before() {
  local branch_label="$1" file="$2" compose_file="$3"
  local repo_url repo_ref source_path compose_project_name network_name
  repo_url="$(get_env_value "${file}" CODEXMANAGER_REPO_URL "")"
  repo_ref="$(get_env_value "${file}" CODEXMANAGER_REPO_REF "")"
  source_path="$(get_env_value "${file}" CODEXMANAGER_SOURCE_PATH "")"
  compose_project_name="$(get_env_value "${file}" CODEXMANAGER_COMPOSE_PROJECT_NAME "")"
  network_name="$(get_env_value "${file}" CODEXMANAGER_NETWORK_NAME "")"

  printf '\n%s 当前部署信息：\n' "${branch_label}"
  printf '  - GitHub 仓库:           %s\n' "${repo_url}"
  printf '  - Git 引用:             %s\n' "${repo_ref}"
  printf '  - 本地源码路径:         %s\n' "${source_path}"
  printf '  - Compose Project:      %s\n' "${compose_project_name}"
  printf '  - 独立网络名:           %s\n' "${network_name}"
  printf '  - Compose 文件:         %s\n' "${compose_file}"
  printf '  - 容器内 API 地址:       %s\n' "$(codexmanager_service_api_base)"
  printf '  - 宿主机 API 地址:       http://127.0.0.1:%s/v1\n' "${DEFAULT_CODEXMANAGER_SERVICE_PORT}"
  printf '  - 宿主机 Web 地址:       http://127.0.0.1:%s/\n' "${DEFAULT_CODEXMANAGER_WEB_PORT}"
  printf '  - 说明: %s会后台启动服务，并在当前终端实时显示日志；按 Ctrl+C 只退出日志视图，不会停止服务。\n\n' "${branch_label}"
}

show_codexmanager_access_info_after() {
  local branch_label="$1" file="$2"
  printf '\n%s 已完成后台部署：\n' "${branch_label}"
  printf '  - 独立网络名:           %s\n' "$(get_env_value "${file}" CODEXMANAGER_NETWORK_NAME "")"
  printf '  - 宿主机 Web 地址:       http://127.0.0.1:%s/\n' "${DEFAULT_CODEXMANAGER_WEB_PORT}"
  printf '  - 宿主机 API 地址:       http://127.0.0.1:%s/v1\n' "${DEFAULT_CODEXMANAGER_SERVICE_PORT}"
  printf '  - 说明: 下面将实时显示 compose 日志；按 Ctrl+C 只退出日志视图，服务会继续在后台运行。\n\n'
}

codexmanager_remote_public_host_label() {
  local file="$1" server_name http_port host_label
  server_name="$(get_env_value "${file}" CODEXMANAGER_REMOTE_SERVER_NAME "${DEFAULT_CODEXMANAGER_REMOTE_SERVER_NAME}")"
  http_port="$(get_env_value "${file}" CODEXMANAGER_REMOTE_HTTP_PORT "${DEFAULT_CODEXMANAGER_REMOTE_HTTP_PORT}")"
  if [[ -z "${server_name}" || "${server_name}" == "_" ]]; then
    host_label="<服务器公网IP或域名>"
  else
    host_label="${server_name}"
  fi
  if [[ "${http_port}" == "80" ]]; then
    printf 'http://%s' "${host_label}"
  else
    printf 'http://%s:%s' "${host_label}" "${http_port}"
  fi
}

show_codexmanager_remote_access_info_before() {
  local branch_label="$1" file="$2" compose_file="$3"
  printf '\n%s 当前远程部署信息：\n' "${branch_label}"
  printf '  - GitHub 仓库:           %s\n' "$(get_env_value "${file}" CODEXMANAGER_REPO_URL "")"
  printf '  - Git 引用:             %s\n' "$(get_env_value "${file}" CODEXMANAGER_REPO_REF "")"
  printf '  - 本地源码路径:         %s\n' "$(get_env_value "${file}" CODEXMANAGER_SOURCE_PATH "")"
  printf '  - Compose Project:      %s\n' "$(get_env_value "${file}" CODEXMANAGER_COMPOSE_PROJECT_NAME "")"
  printf '  - 独立网络名:           %s\n' "$(get_env_value "${file}" CODEXMANAGER_NETWORK_NAME "")"
  printf '  - Compose 文件:         %s\n' "${compose_file}"
  printf '  - Nginx server_name:    %s\n' "$(get_env_value "${file}" CODEXMANAGER_REMOTE_SERVER_NAME "${DEFAULT_CODEXMANAGER_REMOTE_SERVER_NAME}")"
  printf '  - Nginx 对外端口:        %s\n' "$(get_env_value "${file}" CODEXMANAGER_REMOTE_HTTP_PORT "${DEFAULT_CODEXMANAGER_REMOTE_HTTP_PORT}")"
  printf '  - WARP 模式:            %s\n' "$(if [[ "$(get_env_value "${file}" CODEXMANAGER_REMOTE_WARP_ENABLED "0")" == "1" ]]; then printf '开启'; else printf '关闭'; fi)"
  if [[ "$(get_env_value "${file}" CODEXMANAGER_REMOTE_WARP_ENABLED "0")" == "1" ]]; then
    printf '  - WARP/代理 URL:        %s\n' "$(get_env_value "${file}" CODEXMANAGER_GATEWAY_ACCOUNT_PROXY_URL "")"
  fi
  printf '  - 本机检查地址:         http://127.0.0.1:%s/\n' "$(get_env_value "${file}" CODEXMANAGER_REMOTE_HTTP_PORT "${DEFAULT_CODEXMANAGER_REMOTE_HTTP_PORT}")"
  printf '  - 下游接入 /v1:         %s/v1\n' "$(codexmanager_remote_public_host_label "${file}")"
  printf '  - 下游 OAuth 入口:      %s/oauth/authorize\n' "$(codexmanager_remote_public_host_label "${file}")"
  printf '  - 说明: %s会后台启动 Nginx + Web + Service，并在当前终端实时显示日志；按 Ctrl+C 只退出日志视图，不会停止服务。\n\n' "${branch_label}"
}

show_codexmanager_remote_access_info_after() {
  local branch_label="$1" file="$2"
  printf '\n%s 已完成远程部署：\n' "${branch_label}"
  printf '  - 独立网络名:           %s\n' "$(get_env_value "${file}" CODEXMANAGER_NETWORK_NAME "")"
  printf '  - Web 访问地址:         %s/\n' "$(codexmanager_remote_public_host_label "${file}")"
  printf '  - Gateway /v1 地址:     %s/v1\n' "$(codexmanager_remote_public_host_label "${file}")"
  printf '  - OAuth 入口:           %s/oauth/authorize\n' "$(codexmanager_remote_public_host_label "${file}")"
  printf '  - 说明: 下游 CLI 可通过同一域名使用 OAuth / API Key 两种模式；下面将实时显示 compose 日志。\n\n'
}

run_codexmanager_deploy_option() {
  local branch_label="$1" file="$2" default_repo_url="$3" default_repo_ref="$4" default_source_path="$5" default_project_name="$6" target_label="$7"
  local source_path compose_project_name compose_file

  need_cmd docker
  need_cmd git
  need_cmd curl

  collect_codexmanager_runtime_inputs "${file}" "${default_repo_url}" "${default_repo_ref}" "${default_source_path}" "${default_project_name}" "${target_label}"
  prepare_codexmanager_source "${file}"

  source_path="$(normalize_path "$(get_env_value "${file}" CODEXMANAGER_SOURCE_PATH "${default_source_path}")")"
  compose_project_name="$(get_env_value "${file}" CODEXMANAGER_COMPOSE_PROJECT_NAME "${default_project_name}")"
  compose_file="$(resolve_codexmanager_compose_file "${source_path}")" || die "未找到 docker/docker-compose.yml 或 crates/docker/docker-compose.yml"
  ensure_local_codexmanager_ports_available_or_owned_by_project "${compose_project_name}" "${branch_label}"

  show_codexmanager_access_info_before "${branch_label}" "${file}" "${compose_file}"
  (
    cd "${source_path}"
    COMPOSE_PROJECT_NAME="${compose_project_name}" docker compose -f "${compose_file}" up --build -d
  )
  wait_for_codexmanager_stack_ready || warn "${branch_label}部署已启动，但宿主机 Web/API 端口尚未确认可访问，请结合下面日志继续观察。"
  ensure_codexmanager_affinity_defaults || true
  show_codexmanager_access_info_after "${branch_label}" "${file}"
  follow_codexmanager_compose_logs "${source_path}" "${compose_project_name}" "${compose_file}" "${branch_label}" "${file}"
}

run_codexmanager_remote_deploy_option() {
  local branch_label="$1" file="$2" default_repo_url="$3" default_repo_ref="$4" default_source_path="$5" default_project_name="$6" target_label="$7"
  local source_path compose_project_name compose_file http_port

  need_cmd docker
  need_cmd git
  need_cmd curl
  need_cmd sed

  collect_codexmanager_remote_runtime_inputs "${file}" "${default_repo_url}" "${default_repo_ref}" "${default_source_path}" "${default_project_name}" "${target_label}"
  prepare_codexmanager_source "${file}"

  source_path="$(normalize_path "$(get_env_value "${file}" CODEXMANAGER_SOURCE_PATH "${default_source_path}")")"
  compose_project_name="$(get_env_value "${file}" CODEXMANAGER_COMPOSE_PROJECT_NAME "${default_project_name}")"
  compose_file="$(resolve_codexmanager_remote_compose_file "${source_path}")" || die "未找到 docker/docker-compose.remote.yml"
  http_port="$(get_env_value "${file}" CODEXMANAGER_REMOTE_HTTP_PORT "${DEFAULT_CODEXMANAGER_REMOTE_HTTP_PORT}")"
  render_codexmanager_remote_nginx_conf "${file}" "${source_path}"

  show_codexmanager_remote_access_info_before "${branch_label}" "${file}" "${compose_file}"
  (
    cd "${source_path}"
    COMPOSE_PROJECT_NAME="${compose_project_name}" docker compose --env-file "${file}" -f "${compose_file}" up --build -d
  )
  wait_for_codexmanager_remote_stack_ready "${http_port}" || warn "${branch_label}部署已启动，但远程 nginx 检查口尚未确认可访问，请结合下面日志继续观察。"
  show_codexmanager_remote_access_info_after "${branch_label}" "${file}"
  follow_codexmanager_compose_logs "${source_path}" "${compose_project_name}" "${compose_file}" "${branch_label}" "${file}"
}

option_1() {
  generate_files
  collect_build_inputs
  build_image
  printf '\n完成。菜单 1 只负责生成文件并构建 Codex 基础镜像；不会部署 CodexManager，也不会形成 OAuth 实例识别链路。\n'
}

option_2() {
  local codexmanager_access_mode network_name auth_mode codexmanager_remote_base_url go_in
  generate_files
  collect_runtime_inputs
  build_image
  start_container
  codexmanager_access_mode="$(get_env_value "${RUNTIME_ENV_FILE}" CODEXMANAGER_ACCESS_MODE "${DEFAULT_CODEXMANAGER_ACCESS_MODE}")"
  network_name="$(get_env_value "${RUNTIME_ENV_FILE}" NETWORK_NAME "${DEFAULT_NETWORK_NAME}")"
  auth_mode="$(get_env_value "${RUNTIME_ENV_FILE}" CODEX_AUTH_MODE "$(default_codex_auth_mode_for_access_mode "${codexmanager_access_mode}")")"
  codexmanager_remote_base_url="$(get_env_value "${RUNTIME_ENV_FILE}" CODEXMANAGER_REMOTE_BASE_URL "")"
  verify_codex_runtime_against_codexmanager_or_die "${codexmanager_access_mode}" "${network_name}" "${auth_mode}" "${codexmanager_remote_base_url}"
  go_in="$(read_prompt_input "是否现在直接进入容器？[Y/n]: ")"
  if [[ -z "${go_in}" || "${go_in}" =~ ^[Yy]$ ]]; then
    enter_container
  fi
}

option_3() {
  generate_files
  collect_build_inputs
  build_image
}

option_7() {
  warn "当前工作区版 run.sh 未集成 Cockpit Tools 自动部署。"
}

option_10() {
  run_codexmanager_remote_deploy_option \
    "分支10" \
    "${CODEXMANAGER_REMOTE_RUNTIME_ENV_FILE}" \
    "${DEFAULT_CODEXMANAGER_REPO_URL}" \
    "${DEFAULT_CODEXMANAGER_REPO_REF}" \
    "${DEFAULT_CODEXMANAGER_SOURCE_PATH}" \
    "${DEFAULT_CODEXMANAGER_REMOTE_COMPOSE_PROJECT_NAME}" \
    "远程服务器"
}

option_11() {
  install_optional_ollvm_in_container
}

option_8() {
  run_codexmanager_deploy_option \
    "分支8" \
    "${CODEXMANAGER_RUNTIME_ENV_FILE}" \
    "${DEFAULT_CODEXMANAGER_REPO_URL}" \
    "${DEFAULT_CODEXMANAGER_REPO_REF}" \
    "${DEFAULT_CODEXMANAGER_SOURCE_PATH}" \
    "${DEFAULT_CODEXMANAGER_COMPOSE_PROJECT_NAME}" \
    "当前项目"
}

option_9() {
  run_codexmanager_deploy_option \
    "分支9" \
    "${CODEXMANAGER_QXCNM_RUNTIME_ENV_FILE}" \
    "${DEFAULT_CODEXMANAGER_QXCNM_REPO_URL}" \
    "${DEFAULT_CODEXMANAGER_QXCNM_REPO_REF}" \
    "${DEFAULT_CODEXMANAGER_QXCNM_SOURCE_PATH}" \
    "${DEFAULT_CODEXMANAGER_QXCNM_COMPOSE_PROJECT_NAME}" \
    "qxcnm/Codex-Manager"
}

show_menu() {
  cat <<'EOF_MENU'

==============================
 Codex Docker 管理脚本
==============================
1) 构建 Codex 基础镜像（不部署 CodexManager / 不启用 OAuth 实例识别）
2) 启动新 Codex 容器（多实例隔离，支持本地/远端 CodexManager 与 OAuth/API Key）
3) 从失败位置继续构建 Codex 基础镜像
4) 进入当前 Codex 容器
5) 停止并删除当前 Codex 容器
6) 删除 Codex 镜像/关联网络卷
7) 安装并启动 Cockpit Tools 容器
8) 拉取并后台部署当前项目 CodexManager（实时日志）
9) 拉取并后台部署 qxcnm/Codex-Manager（实时日志）
10) 远程服务器 Docker + Nginx 部署（默认亲和度，支持 OAuth/API Key，可选 WARP）
11) 为当前 Codex 容器安装可选 OLLVM 工具链
0) 退出

EOF_MENU
}

main() {
  need_cmd docker
  if ! docker compose version >/dev/null 2>&1; then
    die "当前 Docker 不支持 docker compose。"
  fi

  while true; do
    show_menu
    choice="$(read_prompt_input "请输入选项: ")"
    case "${choice}" in
      1) option_1 ;;
      2) option_2 ;;
      3) option_3 ;;
      4) enter_container ;;
      5) stop_remove_container ;;
      6) remove_image_interactive ;;
      7) option_7 ;;
      8) option_8 ;;
      9) option_9 ;;
      10) option_10 ;;
      11) option_11 ;;
      0) exit 0 ;;
      *) warn "无效选项，请重新输入。" ;;
    esac
  done
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  main "$@"
  exit 0
fi

return 0 2>/dev/null || true
__CODEX_SUPPORT_BUNDLE__
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
