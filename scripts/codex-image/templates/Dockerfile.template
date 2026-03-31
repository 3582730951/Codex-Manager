# syntax=docker/dockerfile:1.7

FROM node:22-bookworm AS skill-builder

ENV DEBIAN_FRONTEND=noninteractive
SHELL ["/bin/bash", "-lc"]

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    git \
    python3 \
    xz-utils \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /tmp/codex-image
COPY scripts/codex-image/install_skill_bundle.py /tmp/codex-image/install_skill_bundle.py
COPY scripts/codex-image/skill_manifest.lock.json /tmp/codex-image/skill_manifest.lock.json
RUN python3 /tmp/codex-image/install_skill_bundle.py \
    --manifest /tmp/codex-image/skill_manifest.lock.json \
    --output-dir /opt/codex-seed \
    --work-dir /tmp/codex-skill-work


FROM node:22-bookworm

ENV DEBIAN_FRONTEND=noninteractive
ENV HOME=/root
ENV CODEX_HOME=/root/.codex
ENV CODEX_MANAGED_SKILLS_ROOT=/root/.agents/skills
ENV CODEX_LEGACY_SKILLS_ROOT=/root/.codex/skills
ENV CODEX_IMAGE_TOOL_ROOT=/opt/codex-image
ENV PLAYWRIGHT_BROWSERS_PATH=/root/.cache/ms-playwright

SHELL ["/bin/bash", "-lc"]

COPY scripts/codex-image /opt/codex-image
RUN bash /opt/codex-image/install_runtime_deps.sh

COPY --from=skill-builder /opt/codex-seed /opt/codex-seed
COPY scripts/codex-entrypoint.sh /usr/local/bin/codex-entrypoint
COPY scripts/codex-login-wrapper.sh /usr/local/bin/codex-wrapper
COPY scripts/codex-oauth-proxy-login.py /usr/local/bin/codex-oauth-proxy-login
RUN install -m 0755 /opt/codex-image/sync_seeded_skills.sh /usr/local/bin/sync-seeded-skills \
  && install -m 0755 /opt/codex-image/prepare_skill_bundle.sh /usr/local/bin/prepare-skill-bundle \
  && install -m 0755 /opt/codex-image/verify_skill_bundle.sh /usr/local/bin/verify-skill-bundle \
  && install -m 0755 /opt/codex-image/install_ollvm_toolchain.sh /usr/local/bin/install-ollvm-toolchain \
  && install -m 0755 /opt/codex-image/akira-clang.sh /usr/local/bin/akira-clang \
  && install -m 0755 /opt/codex-image/akira-clang++.sh /usr/local/bin/akira-clang++ \
  && install -m 0755 /opt/codex-image/ollvm-koto-clang.sh /usr/local/bin/ollvm-koto-clang \
  && mv /usr/local/bin/codex /usr/local/bin/codex-real \
  && install -m 0755 /usr/local/bin/codex-wrapper /usr/local/bin/codex \
  && chmod +x /usr/local/bin/codex-entrypoint /usr/local/bin/codex-oauth-proxy-login

WORKDIR /workspace

ENTRYPOINT ["/usr/local/bin/codex-entrypoint"]
CMD ["sleep", "infinity"]
