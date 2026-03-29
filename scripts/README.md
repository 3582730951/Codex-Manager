# scripts 目录说明

## 分类

### 开发

- `bump-version.ps1`：统一修改版本号
- `codex-image/`：Codex 基础镜像构建支撑层，包含锁定 skills 清单、Docker 模板、依赖安装器、技能同步脚本，以及可选 OLLVM 安装器
- `rebuild.ps1`：Windows 本地桌面构建，也可触发全平台 release workflow
- `rebuild-linux.sh`：Linux 本地桌面构建
- `rebuild-macos.sh`：macOS 本地桌面构建

### 测试

- `tests/chat_tools_hit_probe.ps1`：`/v1/chat/completions` tools 命中探针
- `tests/codex_stream_probe.ps1`：chat / responses 流式探针
- `tests/docker/verify_codex_image.sh`：单个 Codex 容器内验收，覆盖 skills、默认工具链、浏览器，以及可选 OLLVM 烟测
- `tests/docker/verify_codex_image_stack.sh`：在 WSL/Docker 内自建隔离测试容器，执行 `build:desktop + verify_codex_image.sh`
- `tests/gateway_regression_suite.ps1`：协议回归统一入口
- `tests/web_runtime_probe.ps1`：Web 运行壳最小冒烟探针
- `tests/web_ui_smoke.ps1`：Web 管理页页面级冒烟脚本（本地 mock 运行壳）
- `tests/wsl_verify_codex_image.ps1`：从 Windows 宿主经 WSL 调用 Docker，自建隔离 Codex 测试容器并执行完整验收
- `tests/*.test.ps1`：脚本级回归测试

### 发布

- `release/assert-release-version.ps1`
- `release/build-tauri-with-retry.ps1`
- `release/build-tauri-with-retry.sh`
- `release/disable-tauri-before-build.ps1`
- `release/publish-github-release.sh`
- `release/stage-service-package.ps1`
- `release/stage-service-package.sh`

### 仅 CI / workflow 间接调用

以下脚本通常由 workflow 或 composite action 调用，不建议作为日常手工入口：

- `release/build-tauri-with-retry.*`
- `release/stage-service-package.*`
- `release/publish-github-release.sh`
- `release/assert-release-version.ps1`

## 使用建议

1. 本地开发优先用顶层入口脚本，不要直接调用过深的 release 辅助脚本
2. 协议验证优先走 `tests/gateway_regression_suite.ps1`
3. Web 代理、部署或运行壳改动，优先补跑 `tests/web_runtime_probe.ps1`
4. Web 页面兼容或交互降级改动，补跑 `tests/web_ui_smoke.ps1`
5. 若脚本只服务 CI，尽量通过 README 或 workflow 注释说明，不要让它伪装成本地通用入口
6. Codex 基础镜像默认会在容器启动时先准备 skills bundle，再同步到用户作用域 `~/.agents/skills`；`~/.codex/skills` 仅保留给上游 system cache 和旧数据迁移
7. 默认 bundle 额外包含 `3582730951/codex-skills/plan_skills`，其中 `plan-mode-pm-orchestrator` 与 `multi-agent-plan-orchestrator` 会随 `run.sh` 构建的镜像一起预置
8. `CODEX_SKILL_UPDATE_MODE=auto|image|remote` 控制 skills 来源：默认 `auto`，每次启动尝试远端更新，失败时回退到缓存或镜像内置 bundle
9. `CODEX_SKILL_SYNC_MODE=always|smart` 控制同步策略：默认 `always`，每次启动都会重写托管 skills，避免卷内旧内容影响 Codex 读取
10. 默认基础镜像不再内置 OLLVM；需要时在容器内执行 `install-ollvm-toolchain`，或通过 `run.sh` 的可选安装菜单触发
11. 基础镜像会默认安装 `bubblewrap`，避免 Codex 启动时出现 `/usr/bin/bwrap` 缺失警告
12. Codex 镜像改动的最终放行门应走 `tests/wsl_verify_codex_image.ps1`，它会在 WSL -> Docker 中新建测试容器，不复用现有运行容器

## 相关文档

- 测试探针说明：[tests/README.md](tests/README.md)
- 构建发布说明：[../docs/release/20260310122606851_构建发布与脚本说明.md](../docs/release/20260310122606851_构建发布与脚本说明.md)
- 职责对照与盘点：[../docs/report/20260309195735631_脚本与发布职责对照.md](../docs/report/20260309195735631_脚本与发布职责对照.md)
