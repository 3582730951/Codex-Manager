# KEY 配置使用说明（Codex + CodexManager 本地网关）

本文用于说明如何把 Codex 客户端接到本地网关 `http://localhost:48760/v1`，并正确配置平台 Key。

## 1. 先在 CodexManager 里生成平台 Key

1. 打开 CodexManager（EXE）。
2. 确保右上角显示已连接（默认 `localhost:48760`）。
3. 进入「访问密钥」页面。
4. 点击「新增 Key」并复制生成的完整 Key（只显示一次时请立即保存）。

注意：
- 这个 Key 是给客户端访问本地网关用的，不是 OpenAI 官网的 API Key。
- 不要把 Key 发到聊天、截图或公开仓库里。

## 2. 配置 Codex 客户端（config.toml）

编辑文件：`C:\Users\Administrator\.codex\config.toml`

写入或确认以下内容：

```toml
model = "gpt-5.3-codex"
model_provider = "codexmanager"

[model_providers.codexmanager]
name = "CodexManager Local"
base_url = "http://localhost:48760/v1"
wire_api = "responses"
env_key = "CODEXMANAGER_API_KEY"
```

关键点：
- `env_key` 必须是“环境变量名”，不是 Key 本身。
- 例如不能写成：`env_key = "05f7..."`

## 3. 配置环境变量（Windows）

### 仅当前终端会话生效

```powershell
$env:CODEXMANAGER_API_KEY = "这里填你生成的平台Key"
```

### 持久生效（推荐）

```powershell
setx CODEXMANAGER_API_KEY "这里填你生成的平台Key"
```

说明：
- `setx` 设置后，需要重启 Codex 客户端（必要时重开终端）才会读取到新变量。

## 4. 启动与验证

1. 确保 CodexManager 服务在线（`localhost:48760`）。
2. 完全退出并重新打开 Codex 客户端。
3. 发一条测试消息。
4. 在 CodexManager 的请求日志里确认有请求命中。

## 5. 常见报错排查

### 报错：`Missing environment variable: 'xxxxx'`

原因：`env_key` 写成了 Key 值，而不是变量名。  
修复：把 `env_key` 改为 `CODEXMANAGER_API_KEY`，并确保系统里存在该环境变量。

### 报错：`missing api key`（401）

原因：客户端没有携带 Key。  
修复：检查环境变量是否存在，重启客户端。

### 报错：`invalid api key`（403）

原因：Key 不正确，或该 Key 已被禁用。  
修复：在 CodexManager 的「访问密钥」里重新创建并启用 Key。

### 切号看起来不生效

原因通常是请求没有走本地网关，或目标账号不可用。  
修复：确认客户端走的是 `http://localhost:48760/v1`，并在 CodexManager 中查看请求日志与账号状态。

## 6. 安全建议

1. Key 只放环境变量，不要硬编码到脚本和仓库。
2. 发现泄露后立即在 CodexManager 删除旧 Key，并重新生成。
3. 不要在截图中暴露完整 Key 或 Token。
