#!/usr/bin/env bash
set -Eeuo pipefail

export DEBIAN_FRONTEND=noninteractive

log() {
  printf '[install-runtime-deps] %s\n' "$*" >&2
}

retry() {
  local max_attempts="$1"
  local sleep_seconds="$2"
  shift 2

  local attempt=1
  while true; do
    if "$@"; then
      return 0
    fi

    if (( attempt >= max_attempts )); then
      log "command failed after ${attempt} attempts: $*"
      return 1
    fi

    log "command failed (attempt ${attempt}/${max_attempts}); retrying in ${sleep_seconds}s: $*"
    sleep "${sleep_seconds}"
    attempt=$((attempt + 1))
  done
}

download_file() {
  local url="$1"
  local output_path="$2"

  retry 5 10 \
    curl -fL --retry 5 --retry-all-errors --retry-delay 5 \
      --connect-timeout 30 --max-time 1800 \
      -o "${output_path}" "${url}"
}

apt_install() {
  retry 5 10 apt-get install -y --no-install-recommends "$@"
}

configure_google_chrome_repo() {
  local key_path=/tmp/google-linux-signing-key.pub
  install -d -m 0755 /usr/share/keyrings
  download_file https://dl.google.com/linux/linux_signing_key.pub "${key_path}"
  gpg --dearmor -o /usr/share/keyrings/google-linux.gpg "${key_path}"
  cat > /etc/apt/sources.list.d/google-chrome.list <<'EOF_CHROME'
deb [arch=amd64 signed-by=/usr/share/keyrings/google-linux.gpg] http://dl.google.com/linux/chrome/deb/ stable main
EOF_CHROME
}

configure_github_cli_repo() {
  local key_path=/tmp/githubcli-archive-keyring.gpg
  install -d -m 0755 /usr/share/keyrings
  download_file https://cli.github.com/packages/githubcli-archive-keyring.gpg "${key_path}"
  install -m 0644 "${key_path}" /usr/share/keyrings/githubcli-archive-keyring.gpg
  chmod go+r /usr/share/keyrings/githubcli-archive-keyring.gpg
  cat > /etc/apt/sources.list.d/github-cli.list <<EOF_GH
deb [arch=$(dpkg --print-architecture) signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main
EOF_GH
}

install_system_packages() {
  retry 5 10 apt-get update
  apt_install \
    bash \
    binutils \
    bubblewrap \
    build-essential \
    ca-certificates \
    chromium \
    cmake \
    composer \
    curl \
    ffmpeg \
    firefox-esr \
    fluxbox \
    g++ \
    gcc \
    gdb \
    git \
    golang-go \
    gpg \
    gradle \
    imagemagick \
    jq \
    less \
    libffi-dev \
    libssl-dev \
    libreoffice \
    lsb-release \
    make \
    maven \
    ninja-build \
    openssh-client \
    patchelf \
    php-cli \
    php-curl \
    php-gd \
    php-intl \
    php-mbstring \
    php-sqlite3 \
    php-xml \
    php-zip \
    pkg-config \
    poppler-utils \
    procps \
    python3 \
    python3-dev \
    python3-pip \
    python3-venv \
    ripgrep \
    scrot \
    sqlite3 \
    sudo \
    unzip \
    x11-utils \
    xdotool \
    xvfb \
    xz-utils \
    zip
}

install_google_chrome() {
  configure_google_chrome_repo
  retry 5 10 apt-get update
  apt_install google-chrome-stable
}

install_github_cli() {
  configure_github_cli_repo
  retry 5 10 apt-get update
  apt_install gh
}

install_java_21() {
  local archive_path=/tmp/openjdk21.tar.gz
  local java_root=/opt/java
  download_file 'https://api.adoptium.net/v3/binary/latest/21/ga/linux/x64/jdk/hotspot/normal/eclipse?project=jdk' "${archive_path}"
  mkdir -p "${java_root}"
  tar -xzf "${archive_path}" -C "${java_root}"
  local extracted_dir
  extracted_dir="$(find "${java_root}" -mindepth 1 -maxdepth 1 -type d | head -n 1)"
  [[ -n "${extracted_dir}" ]] || {
    echo "Failed to install Java 21." >&2
    return 1
  }
  ln -sf "${extracted_dir}/bin/java" /usr/local/bin/java
  ln -sf "${extracted_dir}/bin/javac" /usr/local/bin/javac
  ln -sf "${extracted_dir}/bin/jar" /usr/local/bin/jar
  cat > /etc/profile.d/java21.sh <<EOF_JAVA
export JAVA_HOME=${extracted_dir}
export PATH="\${JAVA_HOME}/bin:\${PATH}"
EOF_JAVA
}

install_rust() {
  local rustup_script=/tmp/rustup-init.sh
  download_file https://sh.rustup.rs "${rustup_script}"
  retry 3 15 bash "${rustup_script}" -y --profile minimal --component rustfmt --component clippy
  local cargo_bin=/root/.cargo/bin
  for binary in cargo cargo-clippy clippy-driver rustc rustdoc rustfmt rustup; do
    if [[ -x "${cargo_bin}/${binary}" ]]; then
      ln -sf "${cargo_bin}/${binary}" "/usr/local/bin/${binary}"
    fi
  done
}

install_dotnet_10() {
  download_file https://dot.net/v1/dotnet-install.sh /tmp/dotnet-install.sh
  chmod +x /tmp/dotnet-install.sh
  retry 3 15 /tmp/dotnet-install.sh --channel 10.0 --quality ga --install-dir /usr/share/dotnet
  ln -sf /usr/share/dotnet/dotnet /usr/local/bin/dotnet
  cat > /etc/profile.d/dotnet.sh <<'EOF_DOTNET'
export DOTNET_ROOT=/usr/share/dotnet
export PATH="${DOTNET_ROOT}:${PATH}"
EOF_DOTNET
}

install_python_packages() {
  retry 3 15 \
    python3 -m pip install --no-cache-dir --break-system-packages \
    --retries 10 \
    --timeout 600 \
    jupyterlab \
    ipykernel \
    networkx \
    numpy \
    openai \
    openpyxl \
    pandas \
    pdf2image \
    pillow \
    python-pptx \
    uv
}

install_node_tooling() {
  retry 3 15 env \
    npm_config_fetch_retries=5 \
    npm_config_fetch_retry_mintimeout=20000 \
    npm_config_fetch_retry_maxtimeout=120000 \
    npm install -g \
    @openai/codex \
    playwright \
    netlify-cli \
    vercel \
    wrangler
  corepack enable
  retry 3 15 corepack prepare pnpm@latest --activate
  retry 3 15 corepack prepare yarn@stable --activate
  command -v playwright >/dev/null
  retry 3 15 env \
    PLAYWRIGHT_BROWSERS_PATH=/root/.cache/ms-playwright \
    PLAYWRIGHT_DOWNLOAD_CONNECTION_TIMEOUT=600000 \
    playwright install --with-deps chromium firefox webkit
}

cleanup() {
  apt-get clean
  rm -rf /var/lib/apt/lists/* /tmp/* /root/.cache/pip/*
}

main() {
  install_system_packages
  install_google_chrome
  install_github_cli
  install_java_21
  install_rust
  install_dotnet_10
  install_python_packages
  install_node_tooling
  cleanup
}

main "$@"
