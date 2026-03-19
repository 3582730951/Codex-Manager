# scripts/tests 说明

## 分类规则

### 本地手工验证探针

这些脚本主要用于人工联调、连真实服务验证：

- `chat_tools_hit_probe.ps1`
- `chat_tools_hit_probe.cmd`
- `codex_stream_probe.ps1`
- `docker_gateway_probe.sh`
- `gateway_regression_suite.ps1`

特点：

- 需要本地 service 已启动
- 需要真实 `Base` / `ApiKey` / `Model`
- 结果更偏 smoke / compatibility probe，而不是纯离线单元测试

### 可进入 CI 的脚本测试

- `assert-release-version.test.ps1`
- `gateway_regression_suite.test.ps1`
- `rebuild.test.ps1`
- `release_version.test.ps1`

特点：

- 不依赖真实 OpenAI 上游
- 更适合验证参数解析、串联关系、版本约束与脚本返回行为

## 推荐执行顺序

1. 改脚本参数或流程：先跑对应 `.test.ps1`
2. 改协议适配或转发：再跑 `gateway_regression_suite.ps1`
3. 改 Docker / WSL 部署链路：补跑 `docker_gateway_probe.sh`
4. 改 tools/tool_calls：至少补跑 `chat_tools_hit_probe.ps1` 与 `-Stream`
5. 改 responses/chat stream：补跑 `codex_stream_probe.ps1`

## 示例

```powershell
pwsh -NoLogo -NoProfile -File scripts/tests/gateway_regression_suite.ps1 `
  -Base http://localhost:48760 -ApiKey <key> -Model gpt-5.3-codex
```

```bash
./scripts/tests/docker_gateway_probe.sh --rebuild
```

## 维护约定

- 新增真实联调探针时，优先放在本目录并明确参数依赖
- 若脚本可以脱离真实服务运行，应补对应 `.test.ps1`
- 不要把 CI 断言和真实联调逻辑塞进同一个脚本里
