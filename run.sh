#!/usr/bin/env bash
set -Eeuo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUNTIME_ENV_FILE="${PROJECT_DIR}/.runtime.env"
COCKPIT_RUNTIME_ENV_FILE="${PROJECT_DIR}/.cockpit.runtime.env"

DEFAULT_IMAGE_NAME="codex:latest"
DEFAULT_CONTAINER_NAME="codex-e"
DEFAULT_NETWORK_NAME="docker_default"
DEFAULT_API_BASE_URL="https://api.openai.com/v1"
DEFAULT_COMPOSE_PROJECT_NAME="codex"
DEFAULT_PORT_3000="3000"
DEFAULT_PORT_5173="5173"
DEFAULT_PORT_8080="8080"
DEFAULT_WORKSPACE_PATH="/mnt/d/Code/R3_Code/MI/mi_web_proxy"

DEFAULT_COCKPIT_REPO_URL="https://github.com/jlcodes99/cockpit-tools.git"
DEFAULT_COCKPIT_REPO_REF="main"
DEFAULT_COCKPIT_IMAGE_NAME="cockpit-tools:latest"
DEFAULT_COCKPIT_CONTAINER_NAME="cockpit-tools-e"
DEFAULT_COCKPIT_NETWORK_NAME="cockpit_tools_net"
DEFAULT_COCKPIT_COMPOSE_PROJECT_NAME="cockpit-tools"
DEFAULT_COCKPIT_WORKSPACE_PATH="${PROJECT_DIR}/cockpit-workspace"
DEFAULT_COCKPIT_APP_PORT="1420"
DEFAULT_COCKPIT_HMR_PORT="1421"
DEFAULT_COCKPIT_WS_PORT="19528"
DEFAULT_COCKPIT_WS_PROXY_PORT="19529"
DEFAULT_COCKPIT_NOVNC_PORT="6080"
DEFAULT_COCKPIT_VNC_PORT="5901"
DEFAULT_COCKPIT_DISPLAY=":99"
DEFAULT_COCKPIT_HOST_HOME_PATH="${HOME:-}"

COMMUNITY_SKILL_REPOS=(
  "https://github.com/am-will/codex-skills.git|main"
  "https://github.com/am-will/swarms.git|main"
  "https://github.com/dsifry/metaswarm.git|main"
  "https://github.com/grp06/useful-codex-skills.git|main"
  "https://github.com/grp06/codex-bug-hunt-skills.git|main"
  "https://github.com/nextlevelbuilder/ui-ux-pro-max-skill.git|main"
  "https://github.com/twostraws/SwiftUI-Agent-Skill.git|main"
  "https://github.com/AvdLee/SwiftUI-Agent-Skill.git|main"
  "https://github.com/vercel-labs/agent-skills.git|main"
  "https://github.com/callstackincubator/agent-skills.git|main"
  "https://github.com/dhruvanbhalara/skills.git|main"
  "https://github.com/vipulgupta2048/codex-skills.git|main"
)

log() { printf '\n[%s] %s\n' "$(date '+%H:%M:%S')" "$*"; }
warn() { printf '\n[WARN] %s\n' "$*"; }
die() { printf '\n[ERROR] %s\n' "$*" >&2; exit 1; }
need_cmd() { command -v "$1" >/dev/null 2>&1 || die "未找到命令: $1"; }

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

get_runtime_value() { get_env_value "${RUNTIME_ENV_FILE}" "$1" "${2:-}"; }
get_cockpit_runtime_value() { get_env_value "${COCKPIT_RUNTIME_ENV_FILE}" "$1" "${2:-}"; }

upsert_env_value() {
  local file="$1" key="$2" value="$3"
  touch "${file}"
  if grep -qE "^${key}=" "${file}"; then
    sed -i "s|^${key}=.*|${key}=${value}|" "${file}"
  else
    printf '%s=%s\n' "${key}" "${value}" >> "${file}"
  fi
}

prompt_default() {
  local prompt="$1" default_value="$2" input
  read -r -p "${prompt} [默认: ${default_value}]: " input
  if [[ -z "${input}" ]]; then
    printf '%s' "${default_value}"
  else
    printf '%s' "${input}"
  fi
}

prompt_secret_default() {
  local prompt="$1" default_value="$2" input
  if [[ -n "${default_value}" ]]; then
    read -r -s -p "${prompt} [回车沿用上次保存的值]: " input
  else
    read -r -s -p "${prompt}: " input
  fi
  printf '\n' >&2
  if [[ -z "${input}" ]]; then
    [[ -n "${default_value}" ]] && { printf '%s' "${default_value}"; return; }
    die "OPENAI_API_KEY 不能为空。"
  fi
  printf '%s' "${input}"
}

prompt_network_choice() {
  local prompt="$1" default_value="$2" input
  printf '\n%s\n' "${prompt}" >&2
  printf '  1) %s\n' "${DEFAULT_NETWORK_NAME}" >&2
  printf '  2) %s\n' "${DEFAULT_COCKPIT_NETWORK_NAME}" >&2
  printf '  3) 自定义网络名\n' >&2
  read -r -p "请选择 [默认: ${default_value}]: " input
  case "${input}" in
    '') printf '%s' "${default_value}" ;;
    1) printf '%s' "${DEFAULT_NETWORK_NAME}" ;;
    2) printf '%s' "${DEFAULT_COCKPIT_NETWORK_NAME}" ;;
    3) prompt_default "请输入自定义网络名" "${default_value}" ;;
    *) printf '%s' "${input}" ;;
  esac
}

ensure_dirs() {
  mkdir -p "${PROJECT_DIR}/codex-config" "${PROJECT_DIR}/skills" "${PROJECT_DIR}/cockpit-workspace"
}

ensure_named_network() {
  local net_name="$1"
  if ! docker network inspect "${net_name}" >/dev/null 2>&1; then
    log "未检测到网络 ${net_name}，正在创建..."
    docker network create "${net_name}" >/dev/null
    log "已创建网络 ${net_name}"
  fi
}

ensure_runtime_env_exists() { [[ -f "${RUNTIME_ENV_FILE}" ]] || die "未找到 ${RUNTIME_ENV_FILE}，请先执行菜单 1 或 2。"; }
ensure_cockpit_runtime_env_exists() { [[ -f "${COCKPIT_RUNTIME_ENV_FILE}" ]] || die "未找到 ${COCKPIT_RUNTIME_ENV_FILE}，请先执行菜单 7。"; }

image_exists() { docker image inspect "$(get_runtime_value IMAGE_NAME "${DEFAULT_IMAGE_NAME}")" >/dev/null 2>&1; }
cockpit_image_exists() { docker image inspect "$(get_cockpit_runtime_value COCKPIT_IMAGE_NAME "${DEFAULT_COCKPIT_IMAGE_NAME}")" >/dev/null 2>&1; }

compute_cockpit_bridge_urls() {
  local codex_net cockpit_net cockpit_name app_port ws_proxy_port
  codex_net="$(get_runtime_value NETWORK_NAME "${DEFAULT_NETWORK_NAME}")"
  cockpit_net="$(get_cockpit_runtime_value COCKPIT_NETWORK_NAME "${DEFAULT_COCKPIT_NETWORK_NAME}")"
  cockpit_name="$(get_cockpit_runtime_value COCKPIT_CONTAINER_NAME "${DEFAULT_COCKPIT_CONTAINER_NAME}")"
  app_port="$(get_cockpit_runtime_value COCKPIT_APP_PORT "${DEFAULT_COCKPIT_APP_PORT}")"
  ws_proxy_port="$(get_cockpit_runtime_value COCKPIT_WS_PROXY_PORT "${DEFAULT_COCKPIT_WS_PROXY_PORT}")"

  if [[ -n "${codex_net}" && -n "${cockpit_net}" && "${codex_net}" == "${cockpit_net}" ]]; then
    printf 'WEB_URL=http://%s:%s\n' "${cockpit_name}" "${app_port}"
    printf 'WS_URL=ws://%s:%s\n' "${cockpit_name}" "${ws_proxy_port}"
  else
    printf 'WEB_URL=http://host.docker.internal:%s\n' "${app_port}"
    printf 'WS_URL=ws://host.docker.internal:%s\n' "${ws_proxy_port}"
  fi
}

sync_cockpit_bridge_into_codex_runtime() {
  local computed web_url ws_url
  computed="$(compute_cockpit_bridge_urls)"
  web_url="$(printf '%s\n' "${computed}" | awk -F= '/^WEB_URL=/{print $2}')"
  ws_url="$(printf '%s\n' "${computed}" | awk -F= '/^WS_URL=/{print $2}')"
  upsert_env_value "${RUNTIME_ENV_FILE}" COCKPIT_TOOLS_WEB_URL "${web_url}"
  upsert_env_value "${RUNTIME_ENV_FILE}" COCKPIT_TOOLS_WS_URL "${ws_url}"
}

write_community_skill_repo_file() {
  : > "${PROJECT_DIR}/community-skills.repos"
  local entry
  for entry in "${COMMUNITY_SKILL_REPOS[@]}"; do
    printf '%s\n' "${entry}" >> "${PROJECT_DIR}/community-skills.repos"
  done
}

write_community_skill_installer() {
  cat > "${PROJECT_DIR}/install-community-skills.sh" <<'EOF_INSTALLER'
#!/usr/bin/env bash
set -Eeuo pipefail

REPO_FILE="${1:-/tmp/community-skills.repos}"
DEST_DIR="${2:-/opt/community-skills}"
WORK_DIR="/tmp/community-skill-clones"
mkdir -p "${DEST_DIR}" "${WORK_DIR}"

slug_from_url() {
  local url="$1"
  local slug
  slug="$(basename "${url}")"
  slug="${slug%.git}"
  printf '%s' "${slug}"
}

copy_skill_dir() {
  local src_dir="$1" repo_slug="$2"
  local base_name final_name final_path
  base_name="$(basename "${src_dir}")"
  final_name="${base_name}"
  if [[ -e "${DEST_DIR}/${final_name}" ]]; then
    final_name="${repo_slug}-${base_name}"
  fi
  final_path="${DEST_DIR}/${final_name}"
  rm -rf "${final_path}"
  cp -a "${src_dir}" "${final_path}"
}

while IFS='|' read -r repo_url repo_ref; do
  [[ -z "${repo_url}" ]] && continue
  repo_slug="$(slug_from_url "${repo_url}")"
  clone_dir="${WORK_DIR}/${repo_slug}"
  rm -rf "${clone_dir}"
  echo "[community-skills] cloning ${repo_url} (${repo_ref})"
  if ! git clone --depth 1 --branch "${repo_ref}" "${repo_url}" "${clone_dir}"; then
    echo "[community-skills] WARN: clone failed for ${repo_url}" >&2
    continue
  fi

  found=0
  while IFS= read -r skill_file; do
    found=1
    skill_dir="$(dirname "${skill_file}")"
    copy_skill_dir "${skill_dir}" "${repo_slug}"
  done < <(find "${clone_dir}" -type f \( -name 'SKILL.md' -o -name 'skill.md' \) | sort)

  if [[ "${found}" -eq 0 && ( -f "${clone_dir}/SKILL.md" || -f "${clone_dir}/skill.md" ) ]]; then
    copy_skill_dir "${clone_dir}" "${repo_slug}"
    found=1
  fi

  if [[ "${found}" -eq 0 ]]; then
    echo "[community-skills] WARN: no SKILL.md found in ${repo_url}" >&2
  fi
done < "${REPO_FILE}"

find "${DEST_DIR}" -type f \( -name 'SKILL.md' -o -name 'skill.md' \) | sort > "${DEST_DIR}/.installed-skill-index.txt"
echo "[community-skills] installed skill count: $(wc -l < "${DEST_DIR}/.installed-skill-index.txt" | tr -d ' ')"
EOF_INSTALLER
  chmod +x "${PROJECT_DIR}/install-community-skills.sh"
}

write_dockerfile() {
  cat > "${PROJECT_DIR}/Dockerfile" <<'EOF_DOCKERFILE'
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

ARG NODE_VERSION=22.22.0
ARG GO_VERSION=1.22.12
ARG OPENAI_SKILLS_REPO=https://github.com/openai/skills.git
ARG OPENAI_SKILLS_REF=main

ENV LANG=en_US.UTF-8
ENV LANGUAGE=en_US:en
ENV LC_ALL=en_US.UTF-8
ENV JAVA_HOME=/usr/lib/jvm/java-17-openjdk-amd64
ENV PLAYWRIGHT_BROWSERS_PATH=/root/.cache/ms-playwright
ENV PNPM_HOME=/root/.local/share/pnpm
ENV PATH=/root/.cargo/bin:/usr/local/go/bin:/root/.local/bin:${PNPM_HOME}:${PATH}
ENV npm_config_update_notifier=false
ENV npm_config_fund=false
ENV PIP_DISABLE_PIP_VERSION_CHECK=1
ENV PIP_ROOT_USER_ACTION=ignore

RUN apt-get update && apt-get install -y --no-install-recommends \
    bash-completion build-essential ca-certificates cmake composer curl dnsutils fd-find file gdb git gnupg htop iputils-ping jq less locales lsb-release make nano net-tools openssh-client pkg-config procps python3 python3-dev python3-pip python3-venv ripgrep rsync software-properties-common sqlite3 sudo tree tzdata unzip vim wget xdg-utils xz-utils zip zlib1g-dev \
    libasound2 libatk-bridge2.0-0 libatk1.0-0 libbz2-dev libcups2 libdbus-1-3 libdrm2 libffi-dev libgbm1 libgdbm-dev libglib2.0-0 libgtk-3-0 liblzma-dev libncursesw5-dev libnspr4 libnss3 libreadline-dev libsqlite3-dev libssl-dev libx11-xcb1 libxcomposite1 libxdamage1 libxfixes3 libxkbcommon0 libxrandr2 tk-dev uuid-dev fonts-liberation \
    php-bcmath php-cli php-curl php-dev php-intl php-mbstring php-sqlite3 php-xml php-zip openjdk-17-jdk maven gradle \
    && ln -sf /usr/bin/python3 /usr/bin/python \
    && ln -sf /usr/bin/fdfind /usr/local/bin/fd \
    && python3 -m pip install --no-cache-dir --upgrade pip setuptools wheel \
    && python3 -m pip install --no-cache-dir jupyterlab notebook \
    && locale-gen en_US.UTF-8 \
    && update-ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN arch="$(dpkg --print-architecture)" \
    && case "${arch}" in amd64) node_arch="x64" ;; arm64) node_arch="arm64" ;; *) echo "Unsupported architecture: ${arch}" >&2; exit 1 ;; esac \
    && curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${node_arch}.tar.xz" -o /tmp/node.tar.xz \
    && tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 --no-same-owner \
    && rm -f /tmp/node.tar.xz \
    && corepack enable \
    && corepack prepare pnpm@latest --activate \
    && corepack prepare yarn@stable --activate \
    && node --version && npm --version && pnpm --version && yarn --version

RUN npm install -g @openai/codex @playwright/test vercel netlify-cli wrangler && npm cache clean --force

RUN arch="$(dpkg --print-architecture)" \
    && case "${arch}" in amd64) go_arch="amd64" ;; arm64) go_arch="arm64" ;; *) echo "Unsupported architecture: ${arch}" >&2; exit 1 ;; esac \
    && curl -fsSL "https://go.dev/dl/go${GO_VERSION}.linux-${go_arch}.tar.gz" -o /tmp/go.tgz \
    && rm -rf /usr/local/go \
    && tar -C /usr/local -xzf /tmp/go.tgz \
    && rm -f /tmp/go.tgz \
    && /usr/local/go/bin/go version

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile default \
    && /root/.cargo/bin/rustc --version

RUN mkdir -p \
    /root/.cache/pip /root/.cache/ms-playwright /root/.cache/bun /root/.npm /root/.pnpm-store /root/.m2 /root/.composer /root/.cargo /root/go \
    /root/.codex /root/.codex/skills \
    /opt/codex-skills /opt/community-skills /opt/custom-skills /opt/codex-home-seed \
    /workspace

RUN git clone --depth 1 --branch "${OPENAI_SKILLS_REF}" "${OPENAI_SKILLS_REPO}" /tmp/openai-skills \
    && mkdir -p /opt/codex-skills \
    && if [ -d /tmp/openai-skills/skills/.curated ]; then \
         find /tmp/openai-skills/skills/.curated -mindepth 1 -maxdepth 1 -type d \
           -exec bash -lc 'src="$1"; dst="/opt/codex-skills/$(basename "$src")"; rm -rf "$dst"; cp -a "$src" "$dst"' _ {} \; ; \
       fi \
    && if [ -d /tmp/openai-skills/skills/.experimental ]; then \
         find /tmp/openai-skills/skills/.experimental -mindepth 1 -maxdepth 1 -type d \
           -exec bash -lc 'src="$1"; name="$(basename "$src")"; dst="/opt/codex-skills/$name"; if [ -e "$dst" ]; then dst="/opt/codex-skills/experimental-$name"; fi; rm -rf "$dst"; cp -a "$src" "$dst"' _ {} \; ; \
       fi \
    && find /opt/codex-skills -mindepth 2 -maxdepth 2 -type f -name SKILL.md | sort > /opt/codex-skills/.installed-skill-index.txt \
    && rm -rf /tmp/openai-skills

COPY community-skills.repos /tmp/community-skills.repos
COPY install-community-skills.sh /usr/local/bin/install-community-skills.sh
RUN chmod +x /usr/local/bin/install-community-skills.sh \
    && /usr/local/bin/install-community-skills.sh /tmp/community-skills.repos /opt/community-skills

RUN npx -y playwright install --with-deps chromium firefox webkit

COPY codex-config/ /opt/codex-home-seed/
COPY skills/ /opt/custom-skills/
COPY entrypoint.sh /usr/local/bin/entrypoint.sh

RUN chmod +x /usr/local/bin/entrypoint.sh

WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["/bin/bash"]
EOF_DOCKERFILE
}

write_entrypoint() {
  cat > "${PROJECT_DIR}/entrypoint.sh" <<'EOF_ENTRYPOINT'
#!/usr/bin/env bash
set -Eeuo pipefail

version_ge() {
  [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | tail -n 1)" = "$1" ]
}

if [[ -n "${OPENAI_API_BASE:-}" && -z "${OPENAI_BASE_URL:-}" ]]; then export OPENAI_BASE_URL="${OPENAI_API_BASE}"; fi
if [[ -n "${OPENAI_BASE_URL:-}" && -z "${OPENAI_API_BASE:-}" ]]; then export OPENAI_API_BASE="${OPENAI_BASE_URL}"; fi
if [[ -n "${OPENAI_BASE_URL:-}" && -z "${CODEX_API_BASE:-}" ]]; then export CODEX_API_BASE="${OPENAI_BASE_URL}"; fi

export OPENAI_BASE_URL="${OPENAI_BASE_URL:-https://api.openai.com/v1}"
export OPENAI_API_BASE="${OPENAI_API_BASE:-${OPENAI_BASE_URL}}"
export CODEX_API_BASE="${CODEX_API_BASE:-${OPENAI_BASE_URL}}"
export js_repl_node_path="${js_repl_node_path:-$(command -v node)}"

mkdir -p /root/.codex /root/.codex/skills

if [[ -d /opt/codex-home-seed ]]; then
  rsync -a /opt/codex-home-seed/ /root/.codex/
fi

if [[ -d /opt/codex-skills ]]; then
  find /opt/codex-skills -mindepth 1 -maxdepth 1 -type d -exec bash -lc '
    src="$1"
    dst="/root/.codex/skills/$(basename "$src")"
    rm -rf "$dst"
    cp -a "$src" "$dst"
  ' _ {} \;
fi

if [[ -d /opt/community-skills ]]; then
  find /opt/community-skills -mindepth 1 -maxdepth 1 -type d -exec bash -lc '
    src="$1"
    dst="/root/.codex/skills/$(basename "$src")"
    if [[ -e "$dst" ]]; then
      dst="/root/.codex/skills/community-$(basename "$src")"
    fi
    rm -rf "$dst"
    cp -a "$src" "$dst"
  ' _ {} \;
fi

if [[ -d /opt/custom-skills ]]; then
  rsync -a /opt/custom-skills/ /root/.codex/skills/
fi

official_count="$(find /opt/codex-skills -type f -name SKILL.md 2>/dev/null | wc -l | tr -d ' ')"
community_count="$(find /opt/community-skills -type f \( -name 'SKILL.md' -o -name 'skill.md' \) 2>/dev/null | wc -l | tr -d ' ')"
skill_count="$(find /root/.codex/skills -type f \( -name 'SKILL.md' -o -name 'skill.md' \) | wc -l | tr -d ' ')"

echo "Official skills ready: ${official_count}"
echo "Community skills ready: ${community_count}"
echo "Total Codex skills ready: ${skill_count}"
echo "Cockpit bridge (web): ${COCKPIT_TOOLS_WEB_URL:-not-configured}"
echo "Cockpit bridge (ws): ${COCKPIT_TOOLS_WS_URL:-not-configured}"

if command -v node >/dev/null 2>&1; then
  node_version="$(node -v | sed 's/^v//')"
  if ! version_ge "${node_version}" "22.22.0"; then
    echo "WARNING: 当前 Node 版本为 ${node_version}，低于 js_repl 需要的 22.22.0。"
  fi
fi

cd /workspace

if [[ -z "${OPENAI_API_KEY:-}" ]]; then
  echo "WARNING: OPENAI_API_KEY 未设置，Codex 可能无法正常工作。"
fi

exec "$@"
EOF_ENTRYPOINT
  chmod +x "${PROJECT_DIR}/entrypoint.sh"
}

write_compose() {
  cat > "${PROJECT_DIR}/docker-compose.yml" <<'EOF_COMPOSE'
services:
  codex:
    image: ${IMAGE_NAME:-codex:latest}
    build:
      context: .
      dockerfile: Dockerfile
    container_name: ${CONTAINER_NAME:-codex-e}
    working_dir: /workspace
    stdin_open: true
    tty: true
    init: true
    restart: unless-stopped
    networks:
      - codex_net
    extra_hosts:
      - "host.docker.internal:host-gateway"
    ports:
      - "${PORT_3000:-3000}:3000"
      - "${PORT_5173:-5173}:5173"
      - "${PORT_8080:-8080}:8080"
    environment:
      OPENAI_API_KEY: ${OPENAI_API_KEY}
      OPENAI_BASE_URL: ${OPENAI_BASE_URL:-https://api.openai.com/v1}
      OPENAI_API_BASE: ${OPENAI_API_BASE:-https://api.openai.com/v1}
      CODEX_API_BASE: ${CODEX_API_BASE:-https://api.openai.com/v1}
      js_repl_node_path: /usr/local/bin/node
      PLAYWRIGHT_BROWSERS_PATH: /root/.cache/ms-playwright
      PIP_CACHE_DIR: /root/.cache/pip
      npm_config_cache: /root/.npm
      PNPM_STORE_DIR: /root/.pnpm-store
      MAVEN_CONFIG: /root/.m2
      COCKPIT_TOOLS_WEB_URL: ${COCKPIT_TOOLS_WEB_URL:-http://host.docker.internal:1420}
      COCKPIT_TOOLS_WS_URL: ${COCKPIT_TOOLS_WS_URL:-ws://host.docker.internal:19529}
    volumes:
      - ${WORKSPACE_PATH}:/workspace
      - codex_home:/root/.codex
      - npm_cache:/root/.npm
      - pnpm_store:/root/.pnpm-store
      - pip_cache:/root/.cache/pip
      - playwright_cache:/root/.cache/ms-playwright
      - maven_cache:/root/.m2
      - composer_cache:/root/.composer
      - cargo_cache:/root/.cargo
      - go_cache:/root/go
    command: ["/bin/bash"]

volumes:
  codex_home:
  npm_cache:
  pnpm_store:
  pip_cache:
  playwright_cache:
  maven_cache:
  composer_cache:
  cargo_cache:
  go_cache:

networks:
  codex_net:
    external: true
    name: ${NETWORK_NAME:-docker_default}
EOF_COMPOSE
}

write_cockpit_dockerfile() {
  cat > "${PROJECT_DIR}/Dockerfile.cockpit" <<'EOF_COCKPIT_DOCKERFILE'
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive
SHELL ["/bin/bash", "-o", "pipefail", "-c"]

ARG NODE_VERSION=22.22.0

ENV LANG=en_US.UTF-8
ENV LANGUAGE=en_US:en
ENV LC_ALL=en_US.UTF-8
ENV DISPLAY=:99
ENV WEBKIT_DISABLE_DMABUF_RENDERER=1
ENV npm_config_update_notifier=false
ENV npm_config_fund=false

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl file git locales python3 python3-pip build-essential pkg-config socat \
    libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev libgtk-3-dev libglib2.0-dev libsoup-3.0-dev libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev patchelf \
    xvfb fluxbox x11vnc novnc websockify net-tools procps unzip wget xauth x11-utils \
    && locale-gen en_US.UTF-8 \
    && update-ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN arch="$(dpkg --print-architecture)" \
    && case "${arch}" in amd64) node_arch="x64" ;; arm64) node_arch="arm64" ;; *) echo "Unsupported architecture: ${arch}" >&2; exit 1 ;; esac \
    && curl -fsSL "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${node_arch}.tar.xz" -o /tmp/node.tar.xz \
    && tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 --no-same-owner \
    && rm -f /tmp/node.tar.xz \
    && corepack enable \
    && node --version && npm --version

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile default \
    && /root/.cargo/bin/rustc --version

RUN mkdir -p /workspace /tmp/runtime-root /host-home
COPY cockpit-entrypoint.sh /usr/local/bin/cockpit-entrypoint.sh
RUN chmod +x /usr/local/bin/cockpit-entrypoint.sh

WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/cockpit-entrypoint.sh"]
CMD ["bash"]
EOF_COCKPIT_DOCKERFILE
}

write_cockpit_entrypoint() {
  cat > "${PROJECT_DIR}/cockpit-entrypoint.sh" <<'EOF_COCKPIT_ENTRYPOINT'
#!/usr/bin/env bash
set -Eeuo pipefail

REPO_PARENT="/workspace"
REPO_DIR="${COCKPIT_REPO_DIR:-/workspace/cockpit-tools}"
REPO_URL="${COCKPIT_REPO_URL:-https://github.com/jlcodes99/cockpit-tools.git}"
REPO_REF="${COCKPIT_REPO_REF:-main}"
DISPLAY_VALUE="${COCKPIT_DISPLAY:-:99}"
APP_PORT="${COCKPIT_APP_PORT:-1420}"
HMR_PORT="${COCKPIT_HMR_PORT:-1421}"
NOVNC_PORT="${COCKPIT_NOVNC_PORT:-6080}"
VNC_PORT="${COCKPIT_VNC_PORT:-5901}"
WS_PORT="${COCKPIT_WS_PORT:-19528}"
WS_PROXY_PORT="${COCKPIT_WS_PROXY_PORT:-19529}"
HOST_HOME_DIR="${COCKPIT_HOST_HOME_DIR:-/host-home}"

export DISPLAY="${DISPLAY_VALUE}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/runtime-root}"
export TAURI_DEV_HOST="${TAURI_DEV_HOST:-0.0.0.0}"
export HOST="${HOST:-0.0.0.0}"

mkdir -p "${XDG_RUNTIME_DIR}" "${REPO_PARENT}" /root
chmod 700 "${XDG_RUNTIME_DIR}"

for d in .codex .antigravity_cockpit .gemini; do
  if [[ -d "${HOST_HOME_DIR}/${d}" ]]; then
    rm -rf "/root/${d}"
    ln -s "${HOST_HOME_DIR}/${d}" "/root/${d}"
  fi
done

cd "${REPO_PARENT}"

if [[ ! -d "${REPO_DIR}/.git" ]]; then
  echo "[cockpit] cloning ${REPO_URL} (${REPO_REF}) ..."
  rm -rf "${REPO_DIR}"
  git clone --depth 1 --branch "${REPO_REF}" "${REPO_URL}" "${REPO_DIR}"
else
  echo "[cockpit] repo already exists at ${REPO_DIR}, keeping local contents."
fi

cd "${REPO_DIR}"

if [[ -f package-lock.json ]]; then
  echo "[cockpit] installing npm dependencies with npm ci ..."
  npm ci
else
  echo "[cockpit] package-lock.json not found, using npm install ..."
  npm install
fi

echo "[cockpit] starting Xvfb on ${DISPLAY} ..."
Xvfb "${DISPLAY}" -screen 0 1440x900x24 -ac +extension RANDR >/tmp/xvfb.log 2>&1 &

echo "[cockpit] starting fluxbox ..."
fluxbox >/tmp/fluxbox.log 2>&1 &

echo "[cockpit] starting x11vnc on ${VNC_PORT} ..."
x11vnc -display "${DISPLAY}" -forever -shared -rfbport "${VNC_PORT}" -nopw -listen 0.0.0.0 >/tmp/x11vnc.log 2>&1 &

echo "[cockpit] starting noVNC on ${NOVNC_PORT} ..."
/usr/share/novnc/utils/novnc_proxy --listen "${NOVNC_PORT}" --vnc "127.0.0.1:${VNC_PORT}" >/tmp/novnc.log 2>&1 &

echo "[cockpit] starting websocket proxy ${WS_PROXY_PORT} -> 127.0.0.1:${WS_PORT} ..."
socat "TCP-LISTEN:${WS_PROXY_PORT},fork,reuseaddr,bind=0.0.0.0" "TCP:127.0.0.1:${WS_PORT}" >/tmp/ws-proxy.log 2>&1 &

echo "[cockpit] exposed ports: app=${APP_PORT}, hmr=${HMR_PORT}, ws-internal=${WS_PORT}, ws-proxy=${WS_PROXY_PORT}, novnc=${NOVNC_PORT}, vnc=${VNC_PORT}"
echo "[cockpit] starting Tauri dev ..."
exec npm run tauri -- dev
EOF_COCKPIT_ENTRYPOINT
  chmod +x "${PROJECT_DIR}/cockpit-entrypoint.sh"
}
write_cockpit_compose() {
  cat > "${PROJECT_DIR}/docker-compose.cockpit.yml" <<'EOF_COCKPIT_COMPOSE'
services:
  cockpit-tools:
    image: ${COCKPIT_IMAGE_NAME:-cockpit-tools:latest}
    build:
      context: .
      dockerfile: Dockerfile.cockpit
    container_name: ${COCKPIT_CONTAINER_NAME:-cockpit-tools-e}
    working_dir: /workspace
    stdin_open: true
    tty: true
    init: true
    restart: unless-stopped
    networks:
      - cockpit_net
    ports:
      - "${COCKPIT_APP_PORT:-1420}:1420"
      - "${COCKPIT_HMR_PORT:-1421}:1421"
      - "${COCKPIT_WS_PROXY_PORT:-19529}:${COCKPIT_WS_PROXY_PORT:-19529}"
      - "${COCKPIT_NOVNC_PORT:-6080}:6080"
      - "${COCKPIT_VNC_PORT:-5901}:5901"
    environment:
      COCKPIT_REPO_URL: ${COCKPIT_REPO_URL:-https://github.com/jlcodes99/cockpit-tools.git}
      COCKPIT_REPO_REF: ${COCKPIT_REPO_REF:-main}
      COCKPIT_REPO_DIR: /workspace/cockpit-tools
      COCKPIT_DISPLAY: ${COCKPIT_DISPLAY:-:99}
      COCKPIT_APP_PORT: ${COCKPIT_APP_PORT:-1420}
      COCKPIT_HMR_PORT: ${COCKPIT_HMR_PORT:-1421}
      COCKPIT_WS_PORT: ${COCKPIT_WS_PORT:-19528}
      COCKPIT_WS_PROXY_PORT: ${COCKPIT_WS_PROXY_PORT:-19529}
      COCKPIT_NOVNC_PORT: ${COCKPIT_NOVNC_PORT:-6080}
      COCKPIT_VNC_PORT: ${COCKPIT_VNC_PORT:-5901}
      COCKPIT_HOST_HOME_DIR: /host-home
      TAURI_DEV_HOST: 0.0.0.0
      HOST: 0.0.0.0
      WEBKIT_DISABLE_DMABUF_RENDERER: 1
    volumes:
      - ${COCKPIT_WORKSPACE_PATH}:/workspace
      - ${COCKPIT_HOST_HOME_PATH:-/tmp}:/host-home
      - cockpit_npm_cache:/root/.npm
      - cockpit_cargo_home:/root/.cargo
      - cockpit_rustup_home:/root/.rustup
    command: ["bash"]

volumes:
  cockpit_npm_cache:
  cockpit_cargo_home:
  cockpit_rustup_home:

networks:
  cockpit_net:
    external: true
    name: ${COCKPIT_NETWORK_NAME:-cockpit_tools_net}
EOF_COCKPIT_COMPOSE
}

write_codex_config() {
  cat > "${PROJECT_DIR}/codex-config/config.toml" <<'EOF_CONFIG'
[mcp_servers.openaiDeveloperDocs]
url = "https://developers.openai.com/mcp"
EOF_CONFIG

  cat > "${PROJECT_DIR}/codex-config/instructions.md" <<'EOF_INSTR'
Always use the OpenAI developer documentation MCP server if you need to work with the OpenAI API, ChatGPT Apps SDK, Codex, or official OpenAI docs.

General rules:
- Prefer minimal, maintainable changes.
- Prefer the existing package manager and lockfile in the project.
- For frontend debugging, prefer Playwright if the project already uses it.
- Before adding a new dependency, check whether the project already has an equivalent.
- Explain the exact run/test commands after making changes.

Community skills preinstalled in this image include multi-agent orchestration, planning, frontend/UI, SwiftUI, React Native, Flutter, bug-hunting, architecture, and review workflows from public GitHub repositories.

Cockpit Tools bridge:
- Cockpit web URL is exposed through COCKPIT_TOOLS_WEB_URL.
- Cockpit websocket URL is exposed through COCKPIT_TOOLS_WS_URL.
- When Codex and Cockpit share the same Docker network, prefer direct container-to-container URLs.
- When they are on different networks, use host.docker.internal bridge URLs.
EOF_INSTR

  cat > "${PROJECT_DIR}/skills/README.md" <<'EOF_SKILLS'
把你自己的 skill 目录放进这个 skills/ 目录后，重新执行菜单 3 重建镜像，
镜像启动时会自动同步到 /root/.codex/skills。

另外因为 /root/.codex 已经做成了 Docker volume，
所以同一套 compose 环境下，已经安装过的 skills 不需要每个新容器重新安装。
EOF_SKILLS
}

write_dockerignore() {
  cat > "${PROJECT_DIR}/.dockerignore" <<'EOF_IGNORE'
.git
.gitignore
.runtime.env
.cockpit.runtime.env
*.log
node_modules
dist
build
.cache
__pycache__
*.pyc
EOF_IGNORE
}

write_runtime_env() {
  local workspace_path="$1" api_key="$2" image_name="$3" container_name="$4" network_name="$5" api_base_url="$6" cockpit_web_url="$7" cockpit_ws_url="$8"
  cat > "${RUNTIME_ENV_FILE}" <<EOF_RUNTIME
IMAGE_NAME=${image_name}
CONTAINER_NAME=${container_name}
NETWORK_NAME=${network_name}
WORKSPACE_PATH=${workspace_path}

OPENAI_API_KEY=${api_key}
OPENAI_BASE_URL=${api_base_url}
OPENAI_API_BASE=${api_base_url}
CODEX_API_BASE=${api_base_url}

PORT_3000=${DEFAULT_PORT_3000}
PORT_5173=${DEFAULT_PORT_5173}
PORT_8080=${DEFAULT_PORT_8080}
COCKPIT_TOOLS_WEB_URL=${cockpit_web_url}
COCKPIT_TOOLS_WS_URL=${cockpit_ws_url}
EOF_RUNTIME
  chmod 600 "${RUNTIME_ENV_FILE}"
}

write_cockpit_runtime_env() {
  local workspace_path="$1" image_name="$2" container_name="$3" network_name="$4" repo_url="$5" repo_ref="$6" app_port="$7" hmr_port="$8" ws_port="$9" ws_proxy_port="${10}" novnc_port="${11}" vnc_port="${12}" host_home_path="${13}"
  cat > "${COCKPIT_RUNTIME_ENV_FILE}" <<EOF_COCKPIT_RUNTIME
COCKPIT_IMAGE_NAME=${image_name}
COCKPIT_CONTAINER_NAME=${container_name}
COCKPIT_NETWORK_NAME=${network_name}
COCKPIT_WORKSPACE_PATH=${workspace_path}
COCKPIT_REPO_URL=${repo_url}
COCKPIT_REPO_REF=${repo_ref}
COCKPIT_DISPLAY=${DEFAULT_COCKPIT_DISPLAY}
COCKPIT_APP_PORT=${app_port}
COCKPIT_HMR_PORT=${hmr_port}
COCKPIT_WS_PORT=${ws_port}
COCKPIT_WS_PROXY_PORT=${ws_proxy_port}
COCKPIT_NOVNC_PORT=${novnc_port}
COCKPIT_VNC_PORT=${vnc_port}
COCKPIT_HOST_HOME_PATH=${host_home_path}
EOF_COCKPIT_RUNTIME
  chmod 600 "${COCKPIT_RUNTIME_ENV_FILE}"
}

collect_runtime_inputs() {
  local last_workspace last_api_key last_image last_container last_network last_api_base_url
  local workspace_path api_key image_name container_name network_name cockpit_web_url cockpit_ws_url api_base_url computed
  last_workspace="$(get_runtime_value WORKSPACE_PATH "${DEFAULT_WORKSPACE_PATH}")"
  last_api_key="$(get_runtime_value OPENAI_API_KEY "")"
  last_image="$(get_runtime_value IMAGE_NAME "${DEFAULT_IMAGE_NAME}")"
  last_container="$(get_runtime_value CONTAINER_NAME "${DEFAULT_CONTAINER_NAME}")"
  last_network="$(get_runtime_value NETWORK_NAME "${DEFAULT_NETWORK_NAME}")"
  last_api_base_url="$(get_runtime_value OPENAI_BASE_URL "${DEFAULT_API_BASE_URL}")"

  workspace_path="$(prompt_default "请输入工作区挂载路径" "${last_workspace}")"
  api_key="$(prompt_secret_default "请输入 OPENAI_API_KEY" "${last_api_key}")"
  image_name="$(prompt_default "请输入镜像名" "${last_image}")"
  container_name="$(prompt_default "请输入容器名" "${last_container}")"
  network_name="$(prompt_network_choice "请选择 Codex 要加入的网络" "${last_network}")"
  api_base_url="$(prompt_default "请输入 OPENAI_BASE_URL" "${last_api_base_url}")"

  write_runtime_env "${workspace_path}" "${api_key}" "${image_name}" "${container_name}" "${network_name}" "${api_base_url}" "http://host.docker.internal:${DEFAULT_COCKPIT_APP_PORT}" "ws://host.docker.internal:${DEFAULT_COCKPIT_WS_PROXY_PORT}"

  if [[ -f "${COCKPIT_RUNTIME_ENV_FILE}" ]]; then sync_cockpit_bridge_into_codex_runtime; fi

  computed="$(compute_cockpit_bridge_urls 2>/dev/null || true)"
  cockpit_web_url="$(printf '%s\n' "${computed}" | awk -F= '/^WEB_URL=/{print $2}')"
  cockpit_ws_url="$(printf '%s\n' "${computed}" | awk -F= '/^WS_URL=/{print $2}')"
  [[ -z "${cockpit_web_url}" ]] && cockpit_web_url="http://host.docker.internal:${DEFAULT_COCKPIT_APP_PORT}"
  [[ -z "${cockpit_ws_url}" ]] && cockpit_ws_url="ws://host.docker.internal:${DEFAULT_COCKPIT_WS_PROXY_PORT}"
  upsert_env_value "${RUNTIME_ENV_FILE}" COCKPIT_TOOLS_WEB_URL "${cockpit_web_url}"
  upsert_env_value "${RUNTIME_ENV_FILE}" COCKPIT_TOOLS_WS_URL "${cockpit_ws_url}"

  log "已写入 ${RUNTIME_ENV_FILE}"
}

collect_cockpit_runtime_inputs() {
  local last_workspace last_image last_container last_network last_repo_url last_repo_ref
  local last_app_port last_hmr_port last_ws_port last_ws_proxy_port last_novnc_port last_vnc_port last_host_home
  local workspace_path image_name container_name network_name repo_url repo_ref app_port hmr_port ws_port ws_proxy_port novnc_port vnc_port host_home_path

  last_workspace="$(get_cockpit_runtime_value COCKPIT_WORKSPACE_PATH "${DEFAULT_COCKPIT_WORKSPACE_PATH}")"
  last_image="$(get_cockpit_runtime_value COCKPIT_IMAGE_NAME "${DEFAULT_COCKPIT_IMAGE_NAME}")"
  last_container="$(get_cockpit_runtime_value COCKPIT_CONTAINER_NAME "${DEFAULT_COCKPIT_CONTAINER_NAME}")"
  last_network="$(get_cockpit_runtime_value COCKPIT_NETWORK_NAME "${DEFAULT_COCKPIT_NETWORK_NAME}")"
  last_repo_url="$(get_cockpit_runtime_value COCKPIT_REPO_URL "${DEFAULT_COCKPIT_REPO_URL}")"
  last_repo_ref="$(get_cockpit_runtime_value COCKPIT_REPO_REF "${DEFAULT_COCKPIT_REPO_REF}")"
  last_app_port="$(get_cockpit_runtime_value COCKPIT_APP_PORT "${DEFAULT_COCKPIT_APP_PORT}")"
  last_hmr_port="$(get_cockpit_runtime_value COCKPIT_HMR_PORT "${DEFAULT_COCKPIT_HMR_PORT}")"
  last_ws_port="$(get_cockpit_runtime_value COCKPIT_WS_PORT "${DEFAULT_COCKPIT_WS_PORT}")"
  last_ws_proxy_port="$(get_cockpit_runtime_value COCKPIT_WS_PROXY_PORT "${DEFAULT_COCKPIT_WS_PROXY_PORT}")"
  last_novnc_port="$(get_cockpit_runtime_value COCKPIT_NOVNC_PORT "${DEFAULT_COCKPIT_NOVNC_PORT}")"
  last_vnc_port="$(get_cockpit_runtime_value COCKPIT_VNC_PORT "${DEFAULT_COCKPIT_VNC_PORT}")"
  last_host_home="$(get_cockpit_runtime_value COCKPIT_HOST_HOME_PATH "${DEFAULT_COCKPIT_HOST_HOME_PATH}")"

  workspace_path="$(prompt_default "请输入 Cockpit 工作区挂载路径" "${last_workspace}")"
  image_name="$(prompt_default "请输入 Cockpit 镜像名" "${last_image}")"
  container_name="$(prompt_default "请输入 Cockpit 容器名" "${last_container}")"
  network_name="$(prompt_network_choice "请选择 Cockpit Tools 要加入的网络" "${last_network}")"
  repo_url="$(prompt_default "请输入 Cockpit 仓库地址" "${last_repo_url}")"
  repo_ref="$(prompt_default "请输入 Cockpit 分支/标签" "${last_repo_ref}")"
  app_port="$(prompt_default "请输入 Cockpit Vite 端口" "${last_app_port}")"
  hmr_port="$(prompt_default "请输入 Cockpit HMR 端口" "${last_hmr_port}")"
  ws_port="$(prompt_default "请输入 Cockpit 内部 WebSocket 端口" "${last_ws_port}")"
  ws_proxy_port="$(prompt_default "请输入 Cockpit 对外 WebSocket 代理端口" "${last_ws_proxy_port}")"
  novnc_port="$(prompt_default "请输入 Cockpit noVNC 端口" "${last_novnc_port}")"
  vnc_port="$(prompt_default "请输入 Cockpit VNC 端口" "${last_vnc_port}")"
  host_home_path="$(prompt_default "请输入宿主机 HOME 挂载路径（用于映射 .codex/.antigravity_cockpit/.gemini）" "${last_host_home}")"

  mkdir -p "${workspace_path}"
  write_cockpit_runtime_env "${workspace_path}" "${image_name}" "${container_name}" "${network_name}" "${repo_url}" "${repo_ref}" "${app_port}" "${hmr_port}" "${ws_port}" "${ws_proxy_port}" "${novnc_port}" "${vnc_port}" "${host_home_path}"
  log "已写入 ${COCKPIT_RUNTIME_ENV_FILE}"

  if [[ -f "${RUNTIME_ENV_FILE}" ]]; then
    sync_cockpit_bridge_into_codex_runtime
    log "已同步 Cockpit 连接信息到 ${RUNTIME_ENV_FILE}"
  fi
}

generate_files() {
  log "生成项目文件..."
  ensure_dirs
  write_community_skill_repo_file
  write_community_skill_installer
  write_dockerfile
  write_entrypoint
  write_compose
  write_cockpit_dockerfile
  write_cockpit_entrypoint
  write_cockpit_compose
  write_codex_config
  write_dockerignore
  log "项目文件已生成完成。"
}

build_image() {
  need_cmd docker
  ensure_runtime_env_exists
  ensure_named_network "$(get_runtime_value NETWORK_NAME "${DEFAULT_NETWORK_NAME}")"
  log "开始构建 Codex 镜像（包含官方与社区 skills）..."
  DOCKER_BUILDKIT=1 docker compose -p "${DEFAULT_COMPOSE_PROJECT_NAME}" --env-file "${RUNTIME_ENV_FILE}" --progress=plain build
  log "Codex 镜像构建完成。"
}

build_cockpit_image() {
  need_cmd docker
  ensure_cockpit_runtime_env_exists
  ensure_named_network "$(get_cockpit_runtime_value COCKPIT_NETWORK_NAME "${DEFAULT_COCKPIT_NETWORK_NAME}")"
  log "开始构建 Cockpit Tools 镜像..."
  DOCKER_BUILDKIT=1 docker compose -p "${DEFAULT_COCKPIT_COMPOSE_PROJECT_NAME}" -f "${PROJECT_DIR}/docker-compose.cockpit.yml" --env-file "${COCKPIT_RUNTIME_ENV_FILE}" --progress=plain build
  log "Cockpit Tools 镜像构建完成。"
}

start_container() {
  need_cmd docker
  ensure_runtime_env_exists
  ensure_named_network "$(get_runtime_value NETWORK_NAME "${DEFAULT_NETWORK_NAME}")"
  log "启动 Codex 容器..."
  docker compose -p "${DEFAULT_COMPOSE_PROJECT_NAME}" --env-file "${RUNTIME_ENV_FILE}" up -d --force-recreate --remove-orphans
  log "Codex 容器已启动。"
}

start_cockpit_container() {
  need_cmd docker
  ensure_cockpit_runtime_env_exists
  ensure_named_network "$(get_cockpit_runtime_value COCKPIT_NETWORK_NAME "${DEFAULT_COCKPIT_NETWORK_NAME}")"
  log "启动 Cockpit Tools 容器..."
  docker compose -p "${DEFAULT_COCKPIT_COMPOSE_PROJECT_NAME}" -f "${PROJECT_DIR}/docker-compose.cockpit.yml" --env-file "${COCKPIT_RUNTIME_ENV_FILE}" up -d --force-recreate --remove-orphans
  log "Cockpit Tools 容器已启动。"
}

enter_container() {
  ensure_runtime_env_exists
  local container_name="$(get_runtime_value CONTAINER_NAME "${DEFAULT_CONTAINER_NAME}")"
  docker inspect "${container_name}" >/dev/null 2>&1 || die "容器 ${container_name} 不存在。请先执行菜单 2。"
  log "进入容器 ${container_name} ..."
  docker exec -it "${container_name}" bash
}

enter_cockpit_container() {
  ensure_cockpit_runtime_env_exists
  local container_name="$(get_cockpit_runtime_value COCKPIT_CONTAINER_NAME "${DEFAULT_COCKPIT_CONTAINER_NAME}")"
  docker inspect "${container_name}" >/dev/null 2>&1 || die "容器 ${container_name} 不存在。请先执行菜单 7。"
  log "进入 Cockpit 容器 ${container_name} ..."
  docker exec -it "${container_name}" bash
}

stop_remove_container() {
  local container_name="$(get_runtime_value CONTAINER_NAME "${DEFAULT_CONTAINER_NAME}")"
  docker rm -f "${container_name}" >/dev/null 2>&1 || true
  log "容器已删除：${container_name}"
}

remove_image_interactive() {
  ensure_runtime_env_exists
  local default_image image_name confirm
  default_image="$(get_runtime_value IMAGE_NAME "${DEFAULT_IMAGE_NAME}")"
  image_name="$(prompt_default "请输入要删除的镜像名" "${default_image}")"
  docker image inspect "${image_name}" >/dev/null 2>&1 || die "镜像不存在：${image_name}"
  read -r -p "确认删除镜像 ${image_name} ? [y/N]: " confirm
  case "${confirm}" in
    y|Y|yes|YES) docker image rm -f "${image_name}"; log "镜像已删除：${image_name}" ;;
    *) log "已取消删除镜像。" ;;
  esac
}
show_cockpit_access_info() {
  ensure_cockpit_runtime_env_exists
  local app_port hmr_port ws_port ws_proxy_port novnc_port vnc_port codex_web codex_ws
  app_port="$(get_cockpit_runtime_value COCKPIT_APP_PORT "${DEFAULT_COCKPIT_APP_PORT}")"
  hmr_port="$(get_cockpit_runtime_value COCKPIT_HMR_PORT "${DEFAULT_COCKPIT_HMR_PORT}")"
  ws_port="$(get_cockpit_runtime_value COCKPIT_WS_PORT "${DEFAULT_COCKPIT_WS_PORT}")"
  ws_proxy_port="$(get_cockpit_runtime_value COCKPIT_WS_PROXY_PORT "${DEFAULT_COCKPIT_WS_PROXY_PORT}")"
  novnc_port="$(get_cockpit_runtime_value COCKPIT_NOVNC_PORT "${DEFAULT_COCKPIT_NOVNC_PORT}")"
  vnc_port="$(get_cockpit_runtime_value COCKPIT_VNC_PORT "${DEFAULT_COCKPIT_VNC_PORT}")"
  codex_web="$(get_runtime_value COCKPIT_TOOLS_WEB_URL "http://host.docker.internal:${app_port}")"
  codex_ws="$(get_runtime_value COCKPIT_TOOLS_WS_URL "ws://host.docker.internal:${ws_proxy_port}")"

  printf '\nCockpit Tools 当前访问方式：\n'
  printf '  - 宿主机 Web 页面:       http://127.0.0.1:%s\n' "${app_port}"
  printf '  - 宿主机 HMR WebSocket:  ws://127.0.0.1:%s\n' "${hmr_port}"
  printf '  - 宿主机业务 WebSocket:  ws://127.0.0.1:%s （代理到容器内 127.0.0.1:%s）\n' "${ws_proxy_port}" "${ws_port}"
  printf '  - 宿主机 noVNC 桌面:     http://127.0.0.1:%s/vnc.html\n' "${novnc_port}"
  printf '  - 宿主机 VNC 端口:       127.0.0.1:%s\n' "${vnc_port}"
  printf '  - Codex 访问 Web:        %s\n' "${codex_web}"
  printf '  - Codex 访问 WS:         %s\n\n' "${codex_ws}"
}
option_1() {
  generate_files
  collect_runtime_inputs
  build_image
  printf '\n完成。接下来执行菜单 2 就能启动容器。\n'
}

option_2() {
  if [[ ! -f "${PROJECT_DIR}/Dockerfile" || ! -f "${PROJECT_DIR}/docker-compose.yml" ]]; then
    log "检测到项目文件不存在，先自动生成。"
    generate_files
  fi
  collect_runtime_inputs
  if ! image_exists; then
    log "镜像不存在，先自动构建。"
    build_image
  fi
  start_container
  local go_in
  read -r -p "是否现在直接进入容器？[Y/n]: " go_in
  if [[ -z "${go_in}" || "${go_in}" =~ ^[Yy]$ ]]; then enter_container; fi
}

option_3() {
  [[ -f "${PROJECT_DIR}/Dockerfile" && -f "${PROJECT_DIR}/docker-compose.yml" ]] || die "未找到项目文件。请先执行菜单 1。"
  ensure_runtime_env_exists
  log "继续构建。Docker 会复用已经成功的缓存层，从上次失败后的阶段继续。"
  build_image
  local should_start go_in
  read -r -p "构建完成后是否立即启动容器？[Y/n]: " should_start
  if [[ -z "${should_start}" || "${should_start}" =~ ^[Yy]$ ]]; then
    start_container
    read -r -p "是否现在直接进入容器？[Y/n]: " go_in
    if [[ -z "${go_in}" || "${go_in}" =~ ^[Yy]$ ]]; then enter_container; fi
  fi
}

option_7() {
  if [[ ! -f "${PROJECT_DIR}/Dockerfile" || ! -f "${PROJECT_DIR}/docker-compose.yml" ]]; then
    log "检测到 Codex 项目文件不存在，先自动生成。"
    generate_files
  fi
  collect_cockpit_runtime_inputs
  if ! cockpit_image_exists; then
    log "Cockpit 镜像不存在，先自动构建。"
    build_cockpit_image
  else
    local should_rebuild
    read -r -p "检测到 Cockpit 镜像已存在，是否重新构建？[y/N]: " should_rebuild
    if [[ "${should_rebuild}" =~ ^[Yy]$ ]]; then build_cockpit_image; fi
  fi
  start_cockpit_container
  if [[ -f "${RUNTIME_ENV_FILE}" ]]; then
    sync_cockpit_bridge_into_codex_runtime
    log "Codex 运行时环境已自动对接 Cockpit。"
  else
    warn "尚未检测到 ${RUNTIME_ENV_FILE}。你之后执行菜单 1/2 时也会自动带上 Cockpit 对接信息。"
  fi
  show_cockpit_access_info
  local go_in
  read -r -p "是否现在直接进入 Cockpit 容器？[y/N]: " go_in
  if [[ "${go_in}" =~ ^[Yy]$ ]]; then enter_cockpit_container; fi
}

show_menu() {
  cat <<'EOF_MENU'

==============================
 Codex Docker 管理脚本
==============================
1) 初始化并安装（生成文件 + 构建 Codex 镜像，含社区 skills）
2) 启动新 Codex 容器
3) 从失败位置继续安装/构建 Codex
4) 进入当前 Codex 容器
5) 停止并删除当前 Codex 容器
6) 删除 Codex 镜像
7) 安装并启动 Cockpit Tools 容器
0) 退出

EOF_MENU
}

main() {
  need_cmd docker
  if ! docker compose version >/dev/null 2>&1; then die "当前 Docker 不支持 docker compose。"; fi
  while true; do
    show_menu
    read -r -p "请输入选项: " choice
    case "${choice}" in
      1) option_1 ;;
      2) option_2 ;;
      3) option_3 ;;
      4) enter_container ;;
      5) stop_remove_container ;;
      6) remove_image_interactive ;;
      7) option_7 ;;
      0) exit 0 ;;
      *) warn "无效选项，请重新输入。" ;;
    esac
  done
}

main "$@"
