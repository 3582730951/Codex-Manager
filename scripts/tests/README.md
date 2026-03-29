# scripts/tests 说明

## 分类规则

### 本地手工验证探针

这些脚本主要用于人工联调、连真实服务验证：

- `chat_tools_hit_probe.ps1`
- `chat_tools_hit_probe.cmd`
- `codex_stream_probe.ps1`
- `docker/verify_codex_image.sh`
- `docker/verify_codex_image_stack.sh`
- `gateway_regression_suite.ps1`
- `web_runtime_probe.ps1`
- `web_ui_smoke.ps1`
- `wsl_verify_codex_image.ps1`

特点：

- 大多数脚本需要本地 service 已启动
- 大多数脚本需要真实 `Base` / `ApiKey` / `Model`
- 结果更偏 smoke / compatibility probe，而不是纯离线单元测试
- `web_ui_smoke.ps1` 例外：它使用本地 mock Web 运行壳验证页面级兼容，不依赖真实 service
- `verify_codex_image_stack.sh` / `wsl_verify_codex_image.ps1` 例外：它们会主动创建隔离 Codex 测试容器，而不是依赖现有运行容器

### 可进入 CI 的脚本测试

- `assert-release-version.test.ps1`
- `gateway_regression_suite.test.ps1`
- `rebuild.test.ps1`
- `release_version.test.ps1`
- `web_runtime_probe.test.ps1`

特点：

- 不依赖真实 OpenAI 上游
- 更适合验证参数解析、串联关系、版本约束与脚本返回行为

## 推荐执行顺序

1. 改脚本参数或流程：先跑对应 `.test.ps1`
2. 改协议适配或转发：再跑 `gateway_regression_suite.ps1`
3. 改 tools/tool_calls：至少补跑 `chat_tools_hit_probe.ps1` 与 `-Stream`
4. 改 responses/chat stream：补跑 `codex_stream_probe.ps1`
5. 改 Web 运行壳、代理或部署方式：补跑 `web_runtime_probe.ps1`
6. 改 Web 页面兼容、弹窗交互或运行时降级：补跑 `web_ui_smoke.ps1`
7. 改 Codex Docker 镜像、skills 预置或工具链：先跑 `docker/verify_codex_image.sh`
8. 作为最终放行门，补跑 `docker/verify_codex_image_stack.sh`
9. 需要走 Windows -> WSL -> Docker 全链路时：补跑 `wsl_verify_codex_image.ps1`

## 示例

```powershell
pwsh -NoLogo -NoProfile -File scripts/tests/gateway_regression_suite.ps1 `
  -Base http://localhost:48760 -ApiKey <key> -Model gpt-5.3-codex
```

```powershell
pwsh -NoLogo -NoProfile -File scripts/tests/web_runtime_probe.ps1 `
  -Base http://localhost:48761
```

```powershell
pwsh -NoLogo -NoProfile -File scripts/tests/web_ui_smoke.ps1 -SkipBuild
```

```bash
bash scripts/tests/docker/verify_codex_image.sh --container codex-e --with-optional-ollvm
```

```bash
bash scripts/tests/docker/verify_codex_image_stack.sh --with-codex-smoke --model gpt-5.4
```

```powershell
pwsh -NoLogo -NoProfile -File scripts/tests/wsl_verify_codex_image.ps1 -WithCodexSmoke -Model gpt-5.4
```

## 维护约定

- 新增真实联调探针时，优先放在本目录并明确参数依赖
- 若脚本可以脱离真实服务运行，应补对应 `.test.ps1`
- 不要把 CI 断言和真实联调逻辑塞进同一个脚本里
- Codex 镜像验收默认通过 `codex app-server -> skills/list(forceReload=true)` 校验真实发现结果；托管 skills 必须落在 `~/.agents/skills`，旧 `~/.codex/skills` 只能保留 `.system`
- 默认验收固定覆盖 `plan-mode-pm-orchestrator`、`multi-agent-plan-orchestrator` 和 `/usr/bin/bwrap`
- Codex 镜像最终放行要在隔离测试容器里完成，不能只依赖长期运行的本地容器
