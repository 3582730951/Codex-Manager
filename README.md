<p align="center">
  <img src="assets/logo/logo.png" alt="CodexManager Logo" width="220" />
</p>

<h1 align="center">CodexManager</h1>

<p align="center">本地桌面端 + 服务进程的 Codex 账号管理器+网关转发</p>

<p align="center">
  <a href="README.en.md">English</a>
</p>

本地桌面端 + 服务进程的 Codex 账号池管理器，用于统一管理账号、用量与平台 Key，并提供本地网关能力。

## 源码说明：
> 本产品完全由本人指挥+AI打造 Codex（98%） Gemini (2%) 如果在使用过程中产生问题请友好交流，因为开源只是觉得有人能用的上，基本功能也没什么问题，不喜勿喷。
> 其次是本人没有足够的环境来验证每个包都有没有问题，本人也要上班(我只是个穷逼买不起mac之类的)，本人只保证win的桌面端的可用性，如果其他端有问题，请在交流群反馈或者在充分测试后提交Issues，有时间我自会处理
> 最后感谢各位使用者在交流群反馈的各个平台的问题和参与的部分测试。


## 免责声明

- 本项目仅用于学习与开发目的。

- 使用者必须遵守相关平台的服务条款（例如 OpenAI、Anthropic）。

- 作者不提供或分发任何账号、API Key 或代理服务，也不对本软件的具体使用方式负责。

- 请勿使用本项目绕过速率限制或服务限制。

## 首页导览
| 你要做什么 | 直接进入 |
| --- | --- |
| 首次启动、部署、Docker、macOS 放行 | [运行与部署指南](docs/report/20260310122606850_运行与部署指南.md) |
| 配置端口、代理、数据库、Web 密码、环境变量 | [环境变量与运行配置](docs/report/20260309195355187_环境变量与运行配置说明.md) |
| 排查账号不命中、导入失败、挑战拦截、请求异常 | [FAQ 与账号命中规则](docs/report/20260310122606852_FAQ与账号命中规则.md) |
| 本地构建、打包、发版、脚本调用 | [构建发布与脚本说明](docs/release/20260310122606851_构建发布与脚本说明.md) |

## 最近变更
- 当前最新版本：`v0.1.10`（2026-03-18）
- `v0.1.10` 是基于 `v0.1.9` 的补发修复版，重点修复 Web / Docker 误提示桌面专属能力、账号启用 / 禁用参数错误、禁用账号仍被轮询、`refresh token 401` 状态不统一，以及 Windows 本地 Web 启动器关闭后子进程残留后台的问题。
- 上一个大版本更新仍然是把桌面端和 Web 管理界面整体重做并收口到新的 `apps` 前端：旧前端已移除，账号管理、平台密钥、请求日志、设置页、顶部状态栏和侧边导航都换成统一的桌面优先布局，列表密度、弹窗交互、筛选区和卡片区也做了整轮重构。
- 请求链路继续按 Codex 实际行为收口，但只保留真正影响请求命中的部分：登录 / callback / workspace 校验、refresh 语义、`/v1/responses` 与 `/v1/responses/compact` 的请求体重写、线程锚点、`session_id` / `x-client-request-id` / `x-codex-turn-state`、请求压缩、错误摘要和 fallback 诊断都已补齐。
- 账号策略与可用性也做了实用收口：free / 7 天单窗口账号现在会统一按设置里的模型发起请求；优先账号、失败回退、并发上限和 refresh token 误摘号问题都做了修正，请求日志也能看到首尝试账号与尝试链路。
- 可观测性明显增强：请求日志改为后端分页与后端统计，compact 假成功体、HTML/challenge 页、`401 refresh` 原因、`503 no available account` 等失败场景都会写出更明确的诊断信息，网关磁盘日志也收敛成失败摘要导向。
- 桌面稳定性和启动体验继续修过一轮：服务启动误判、`/rpc` 空响应、刷新用量弹窗不更新、首次切页卡顿、Hydration 不一致、开发态渲染指示误导等问题都已处理，Web 密码和桌面/Web 设置同步也已收口。
- 发布链路也做了统一治理：版本已提升到 `0.1.10`，Tauri Rust 侧和 workflow 里的 Tauri CLI / pnpm 版本已重新对齐，`release-all.yml` 继续作为 Windows / macOS / Linux 的单一发布入口。完整历史请看 [CHANGELOG.md](CHANGELOG.md)。
- 未发版更新：
  - 账号管理新增封禁识别与封禁筛选；`account_deactivated` 与 `workspace_deactivated` 会被自动标记为不可用，并可在列表里直接筛选“封禁”。
  - 账号页 5 小时 / 7 天额度下方新增重置时间展示；对于仅提供 7 天窗口的 free 账号，重置时间会跟随显示到 7 天列。
  - 平台密钥新增服务等级配置：`跟随请求`、`Fast`、`Flex`。其中 `Fast` 会映射到上游 `priority`，`Flex` 会直传为 `flex`。
  - 平台密钥桌面端创建 / 编辑链路已修正，协议、模型、推理等级、服务等级现在都能正常保存与回显。
  - 设置页补回服务监听切换，支持在 `localhost` 与 `0.0.0.0` 之间切换；同时修正“检查更新”按钮仅在手动点击时显示加载状态。

### 近期提交摘要
- `9435be2`：新增外观版本切换。设置页现在支持“默认 / 渐变版本”两套视觉预设，支持即时切换、持久化保存，并同步收口了默认值、卡片尺寸和切换行为。
- `cf351e4`：修复发布缓存并优化 Docker 配置。发布 workflow 避免在不执行 `pnpm install` 的构建 job 中错误开启 pnpm 缓存，同时补充了 Docker 运行用户、健康检查、构建上下文裁剪和 compose 依赖顺序。
- `7f6aa6b`：统一主题样式并修复发布细节。主题变量、玻璃卡片层次、背景渐层和设置页外观整体做了统一收口，并顺带修正发布流程里的若干细节问题。
- `70c1ee7`：修复发布工作流 Node 与 Tauri CLI 版本。重新对齐 workflow 里 Node、pnpm 与 Tauri CLI 版本，降低跨平台打包时的版本漂移风险。
- `1fafcf9`：调整免责声明与搜索框样式。补强顶部免责声明展示，同时微调搜索框与界面细节，减少桌面端视觉噪音。
- `43530c1`：补充 README 交流圈二维码。文档里已增加交流圈入口，便于集中反馈、交流和跟进问题。



## 功能概览
- 账号池管理：分组、标签、排序、备注、封禁识别与封禁筛选
- 批量导入 / 导出：支持多文件导入、桌面端文件夹递归导入 JSON、按账号导出单文件
- 用量展示：兼容 5 小时 + 7 日双窗口，以及仅返回 7 日单窗口的账号，并展示对应窗口的重置时间
- 授权登录：浏览器授权 + 手动回调解析
- 平台 Key：生成、禁用、删除、模型绑定、推理等级、服务等级（跟随请求 / Fast / Flex）
- 本地服务：自动拉起、可自定义端口与监听地址
- 本地网关：为 CLI 和第三方工具提供统一 OpenAI 兼容入口

## 截图
![仪表盘](assets/images/dashboard.png)
![账号管理](assets/images/accounts.png)
![平台 Key](assets/images/platform-key.png)
![日志视图](assets/images/log.png)
![设置页](assets/images/themes.png)

## 快速开始
1. 启动桌面端，点击“启动服务”。
2. 进入“账号管理”，添加账号并完成授权。
3. 如回调失败，粘贴回调链接手动完成解析。
4. 刷新用量并确认账号状态。

## 页面展示
### 桌面端
- 账号管理：集中导入、导出、刷新账号与用量，支持低配额 / 封禁筛选与重置时间展示
- 平台 Key：按模型、推理等级、服务等级绑定平台 Key，并查看调用日志
- 设置页：统一管理端口、监听地址、代理、主题、自动更新、后台行为

### Service 版
- `codexmanager-service`：提供本地 OpenAI 兼容网关
- `codexmanager-web`：提供浏览器管理页面
- `codexmanager-start`：一键拉起 service + web

## 常用文档
- 版本历史：[CHANGELOG.md](CHANGELOG.md)
- 协作约定：[CONTRIBUTING.md](CONTRIBUTING.md)
- 架构说明：[ARCHITECTURE.md](ARCHITECTURE.md)
- 测试基线：[TESTING.md](TESTING.md)
- 安全说明：[SECURITY.md](SECURITY.md)
- 文档索引：[docs/README.md](docs/README.md)

## 专题页面
| 页面 | 内容 |
| --- | --- |
| [运行与部署指南](docs/report/20260310122606850_运行与部署指南.md) | 首次启动、Docker、Service 版、macOS 放行 |
| [环境变量与运行配置](docs/report/20260309195355187_环境变量与运行配置说明.md) | 应用配置、代理、监听地址、数据库、Web 安全 |
| [FAQ 与账号命中规则](docs/report/20260310122606852_FAQ与账号命中规则.md) | 账号命中、挑战拦截、导入导出、常见异常 |
| [最小排障手册](docs/report/20260307234235414_最小排障手册.md) | 快速定位服务启动、请求转发、模型刷新异常 |
| [构建发布与脚本说明](docs/release/20260310122606851_构建发布与脚本说明.md) | 本地构建、Tauri 打包、Release workflow、脚本参数 |
| [发布与产物说明](docs/release/20260309195355216_发布与产物说明.md) | 各平台发版产物、命名、是否 pre-release |
| [脚本与发布职责对照](docs/report/20260309195735631_脚本与发布职责对照.md) | 各脚本负责什么、什么场景该用哪个 |
| [协议兼容回归清单](docs/report/20260309195735632_协议兼容回归清单.md) | `/v1/chat/completions`、`/v1/responses`、tools 回归项 |
| [CHANGELOG.md](CHANGELOG.md) | 最新发版内容、未发版更新与完整版本历史 |

## 目录结构
```text
.
├─ apps/                # 前端与 Tauri 桌面端
│  ├─ src/
│  ├─ src-tauri/
│  └─ dist/
├─ crates/              # Rust core/service
│  ├─ core
│  ├─ service
│  ├─ start              # Service 版本一键启动器（拉起 service + web）
│  └─ web                # Service 版本 Web UI（可内嵌静态资源 + /api/rpc 代理）
├─ docs/                # 正式文档目录
├─ scripts/             # 构建与发布脚本
└─ README.md
```

## 鸣谢与参考项目

- Codex（OpenAI）：本项目在请求链路、登录语义与上游兼容行为上参考了该项目的实现与源码结构 <https://github.com/openai/codex>

## 认可社区
<p align="center">
  <a href="https://linux.do/t/topic/1688401" title="LINUX DO">
    <img
      src="https://cdn3.linux.do/original/4X/d/1/4/d146c68151340881c884d95e0da4acdf369258c6.png?style=for-the-badge&logo=discourse&logoColor=white"
      alt="LINUX DO"
      width="100"
      hight="100"
    />
  </a>
</p>

## 联系方式
- 公众号：七线牛马
- 微信： ProsperGao

- 交流群：答案是项目名：CodexManager

  <img src="assets/images/qq_group.jpg" alt="交流群二维码" width="280" />
