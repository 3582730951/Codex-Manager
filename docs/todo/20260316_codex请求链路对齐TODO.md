# Codex 请求链路对齐 TODO

更新时间：2026-03-17

## 结论

当前产品主线应切回：

- 只对齐真正影响 `Codex -> CodexManager -> chatgpt.com/backend-api/codex` 请求链路的部分
- 不继续把 `CodexManager` 扩成完整 `codex-rs/app-server` 兼容实现
- 不为了“方法名看起来一致”去补 `thread/*`、`turn/*`、`review/*`、`skills/*`、`plugin/*`、`app/*`、`config/*` 等非核心协议面

原因：

- `CodexManager` 的产品定位是多账号网关 + 桌面管理器，不是官方 app-server 替身
- 这些方法面对当前桌面产品没有直接用户价值
- 继续补全协议面会明显增加启动、RPC、运行时复杂度，且容易引入无意义故障
- 真正影响可用性、Cloudflare 触发率和请求成功率的，主要还是请求链路本身

## 当前应保留的已完成对齐

### P0 已完成，继续保留

- `ChatGPT OAuth` 登录闭环
- token 刷新与账号计划类型识别
- `/v1/responses` 请求改写与流式桥接
- `/v1/responses/compact` 上游路径和非流式 JSON 语义
- `session_id`
- `x-client-request-id`
- `x-openai-subagent`
- `x-codex-turn-state`
- `conversation_id -> prompt_cache_key` 的线程锚点补齐
- 动态 `Originator / User-Agent / Residency`
- `/responses` 流式请求体 `zstd` 压缩
- free / 单 7 天窗口账号的模型改写与候选策略
- 请求日志里的首尝试账号、尝试链路和失败原因

### P1 已完成，按收益保留

- 启动阶段 `POST /rpc` 直连前置代理，避免空响应误判
- 桌面端 `service_initialize` / `startup_snapshot` 的运行时环境注入
- 启动错误态自动恢复重试

说明：

- 上述项目虽然不都属于“上游请求形状”，但都直接影响桌面端把请求成功发出去，属于当前主线。

## 当前真正需要继续补的请求链路

### 1. 登录与鉴权 on-wire 对齐

目标：

- 对齐官方登录回调、token 交换、错误模型和请求头
- 继续减少“同账号在 Codex 成功、在 CodexManager 容易 challenge / 失效误判”的差异

待做：

- [x] 登录启动不再失败放行；本地 callback server 起不来时直接失败
- [x] callback 增加 `error / error_description / state` 错误建模
- [x] callback 成功后增加 workspace 二次校验
- [x] 浏览器授权 `scope` 对齐官方 connectors scope
- [x] 授权码换 token 与 id_token 换 API key 的请求头已收回到官方登录服务器形状，不再额外挂 `Originator / User-Agent / Residency`
- [x] refresh token 请求体改成官方 `application/json` 形状
- [x] usage endpoint 请求头统一到 `ChatGPT-Account-ID` 语义，并对 challenge / HTML 失败输出稳定摘要
- [x] `CPA no cookie` 模式与 `ChatGPT-Account-ID` 解耦，只抑制 cookie/粘性头，不再误去掉账号头
- [x] `CPA no cookie` 模式下，`/responses` 与 `/responses/compact` 都会真正停止发送上游 `Cookie`
- [x] `planType` 读取优先按最新 access token claims
- [x] 对外 `planType` 已统一到官方枚举语义，并对未知值保留原始 plan 供诊断
- [x] 401 refresh 错误文案映射到官方 expired / reused / revoked / unknown 消息
- [x] refresh `401` 内部原因收口到稳定枚举，避免后续只靠散乱字符串匹配
- [x] token endpoint 错误解析贴近官方优先级，并对 transport error 做敏感 URL 脱敏
- [x] token endpoint 遇到 challenge / HTML 页面时输出稳定摘要，不再原样透传整页 HTML
- [x] token endpoint 命中 `403 + Cloudflare blocked` 时输出官方风格 blocked 摘要
- [x] token endpoint / api key exchange 失败摘要补齐 `request_id / cf-ray / auth_error` 调试头
- [x] token endpoint / api key exchange 的 `x-error-json` 统一支持原始 JSON 与 base64 两种头值，并补齐 `identity_error_code`
- [x] 当 refresh `401` 的 body 缺少错误码时，继续从 `x-error-json / x-openai-authorization-error` 头部兜底判定 canonical 原因
- [ ] 继续复核登录回调与 token 链的剩余请求头使用点
- [x] 对齐 token endpoint 错误解析，继续细化 challenge / HTML / 非 JSON 子类，并让空 body 场景继续从 `auth_error / identity_error_code` 头部推断 `blocked / identity_error / auth_error / cloudflare_edge` 类型
- [x] 复核 refresh token 失败后的账号状态迁移，继续避免误摘号
- [x] 收紧 refresh 失效判定：仅 401 视为 refresh 认证失败，403/挑战页/代理异常不再摘号

验收：

- 桌面端登录、刷新、重登不会因为误判把账号批量摘掉
- 登录相关错误文案能区分 token 失效、挑战页、代理异常、端口异常

### 2. `/responses` 主链路对齐

目标：

- 让 `POST /v1/responses` 的实际出站请求尽量贴近官方 Codex

待做：

- [x] 收掉 `tool_choice=auto`、`reasoning.encrypted_content` 这类官方默认值差异
- [x] 模型列表 `/models` 请求头收回到与官方默认客户端一致的 `originator / User-Agent / ChatGPT-Account-ID / residency` 语义，并移除历史 `Version` 头
- [x] 模型列表 `/models` 失败诊断收口到稳定 challenge / HTML / auth / `identity_error_code` 摘要，并保持 OpenAI fallback 触发条件兼容
- [x] 模型列表 `/models` 不再显式发送上游 `Cookie`
- [ ] 继续核对请求体字段白名单和默认值的剩余边角
- [x] 对齐流式与非流式的 header profile 分支
- [x] 收掉 HTTP `/responses` 上不该显式发送的 `Conversation_id / OpenAI-Beta / Connection / Version`
- [x] 当上游目标是 `api.openai.com/v1` 时，`/responses` 不再透传 ChatGPT 侧 `Cookie / ChatGPT-Account-ID`
- [x] 透传官方 `x-codex-beta-features`
- [x] 透传官方 `x-codex-turn-metadata`（仅 ASCII 安全值）
- [x] 客户端未传 `x-client-request-id` 时，仅在线程锚点（`prompt_cache_key / Conversation_id`）存在时自动补齐，不再从普通 `session_id` 派生
- [x] 当 `/responses` 已有 `prompt_cache_key` 时，让 `session_id / x-client-request-id` 优先跟随线程锚点，不再让旧 `Session_id` 抢占
- [x] 当入站明确携带 `Conversation_id` 时，让线程锚点覆盖旧 `x-client-request-id`
- [x] 当旧 `Session_id` 已被新的线程锚点覆盖时，丢弃旧 `x-codex-turn-state`
- [x] 收掉 `/responses` 主链上 remote 地址参与 `session_id` 派生的兼容分支；保留基于账号/密钥的稳定 session 兼容
- [x] 收紧 `x-codex-turn-state` 的入站信任：缺少稳定线程锚点时不再盲信客户端自带 turn-state
- [x] 把 `openai fallback` 的线程锚点、`session_id`、`x-client-request-id` 语义继续收齐到主链
- [x] fallback 分支在缺少稳定线程锚点时，也不再信任孤立的 `x-codex-turn-state`
- [ ] 复核失败重试、failover、日志落盘时机，避免多账号切换误导

验收：

- 同一账号同一模型下，CodexManager 的出站请求形状与官方 Codex 差异可收敛到少量可解释字段

### 3. `/responses/compact` 远端压缩链路对齐

目标：

- 保持当前 compact 路由、请求体和头语义正确
- 只补真正影响远端 compaction 成功率的部分

待做：

- [x] compact 默认补 `x-openai-subagent=compact`，与官方 `compact_remote` 保持一致
- [x] compact 不再显式发送上游 `Cookie`
- [x] compact 在入站带 `Conversation_id` 时，会上游发送 `session_id=<conversation_id>`，不再让旧 `session_id` 抢占线程锚点
- [ ] 继续核对 compact 其余专用头部与 `session_id` 兼容分支
- [x] compact 上游 `2xx` 假成功体改判为 `502`，避免 HTML/challenge/异常 JSON 透传成功
- [x] compact 上游 `403/5xx` 的 HTML/challenge 页改成结构化 JSON 错误返回，不再透传整页 HTML
- [x] compact 结构化错误补齐稳定 `kind` 分类，能直接区分 `cloudflare_challenge / cloudflare_blocked / identity_error / auth_error / cloudflare_edge / html / invalid_success_body / non_json`
- [x] compact 在 body 为空、但头部已给出 `auth_error / identity_error_code` 时，也会强制走结构化错误返回
- [ ] 继续细化 compact 失败时的 fallback 与其余调试头诊断
- [ ] 如果官方 `compact_remote` 的历史替换行为会影响真实请求链路，再按需补对应状态传递；否则不补 `thread/compact/start`

验收：

- `/v1/responses/compact` 能稳定命中上游真实 `/responses/compact`
- 失败时能明确区分 challenge、账号风控、请求形状差异

### 4. WebSocket / prewarm / reuse

目标：

- 只在它真正影响上游 `responses` 主链路时推进

说明：

- 这项不是“为了补 app-server”
- 而是因为官方 `core` 在常规任务链路里确实用了 `responses websocket / prewarm / reuse`

待做：

- [ ] 核清当前官方哪些模型 / provider / 配置下会优先走 websocket
- [ ] 评估是否需要在网关层补“上游 responses websocket”而不是本地 RPC websocket
- [ ] 若确认确有收益，再做最小实现；否则明确记录为暂不实施

验收：

- 只有在能证明对请求成功率或挑战概率有收益时，才进入实现

### 5. 请求失败诊断链路

目标：

- 失败时能直接看出是请求形状、账号、代理、Cloudflare、上游中断，还是本地桥接问题

待做：

- [x] `gateway-trace.log` 已对失败请求补齐 `REQUEST_START / CANDIDATE_* / ATTEMPT_* / BRIDGE_RESULT / FAILED_REQUEST` 上下文缓冲，成功请求仍不落 trace 文件
- [x] `BRIDGE_RESULT` 与失败日志已补充 `request_id / cf-ray / content-type` 诊断摘要
- [x] `/responses` 与 `compact` 的 challenge / HTML 失败摘要补齐 `auth_error`
- [x] `/responses` 与 `compact` 的 `x-error-json` 已统一支持原始 JSON / base64 两种头值，并补齐 `identity_error_code`
- [ ] 继续增强 `gateway-trace.log` 对最后一帧、最后一跳、响应头、body 摘要的记录
- [ ] 对 403/502/503 建立更稳定的错误分类
- [ ] 让桌面端 toast 和请求日志错误文案尽量使用同一错误源

验收：

- 遇到失败时，不再需要同时翻多份日志才能判断主因

## 明确不再继续对齐的范围

下列内容当前不作为主线目标：

- `thread/start`
- `thread/resume`
- `thread/fork`
- `thread/read`
- `thread/name/set`
- `thread/compact/start`
- `thread/realtime/*`
- `turn/start`
- `turn/steer`
- `turn/interrupt`
- `review/start`
- `skills/*`
- `plugin/*`
- `app/*`
- `config/*`
- `mcpServer/*`
- `externalAgentConfig/*`
- `account/*` 通知流
- 本地 `/rpc/events` SSE 兼容层
- 本地 `GET /rpc` WebSocket app-server 兼容层

说明：

- 这些能力不是“永远不做”
- 而是当前没有足够产品价值，不应该继续消耗主线开发成本
- 如果后续真要做，也应以“服务某个明确产品能力”为前提，而不是为了协议看起来更像官方

## 当前本地未提交改动的处理建议

这批文件属于“全协议对齐扩展”，当前不建议继续推进到主线：

- `crates/service/src/thread_turn/mod.rs`
- `crates/service/src/thread_turn/store.rs`
- `crates/service/src/thread_turn/types.rs`
- `crates/service/src/rpc_dispatch/thread_turn.rs`
- `crates/service/src/rpc_dispatch/codex_compat.rs`
- `crates/service/src/http/tests/proxy_runtime_tests.rs`
- `crates/service/tests/rpc.rs`

处理建议：

- 不继续往这些文件上叠功能
- 不以这些能力作为后续“官方对齐”的完成标准
- 后续如需提交，应先重新评估哪些改动确实服务请求链路，哪些应拆掉或单独搁置

## 下一步实施顺序

1. 只看登录、token、`/responses`、`/responses/compact`、请求头、压缩、失败日志
2. 对照官方 `core/client.rs`、`default_client.rs`、`auth.rs`、`compact_remote.rs` 做 on-wire 复核
3. 产出一份“请求链路差异清单”
4. 按收益从高到低补：
   - 登录与 token
   - `/responses`
   - `/responses/compact`
   - 失败诊断
   - 再决定 websocket / prewarm / reuse 要不要进
