# Codex 开源实现调研

调研日期：2026-10-21。这里记录的是我们开始实现时需要依赖的架构事实，来源以 OpenAI 官方仓库和官方文档为准。

## 结论

Codex 是目前最适合用作第一条基线的方案。OpenAI 将 CLI、核心 agent harness、app-server 和 SDK 作为开源组件发布；主仓库使用 Apache-2.0。核心实现是 Rust workspace，产品界面和 agent runtime 分层清晰。

直接 fork 整个仓库可以最快获得一个可用产品，但会立即继承较大的工程复杂度。更稳妥的研究方式是：先把官方版本跑通并记录行为，再复现最小闭环，最后按需要移植其成熟的沙箱、协议和持久化设计。

## 运行时模型

Codex 的基础实体可以归纳为：

```text
Client / TUI / SDK
        │ submissions and streamed events
        ▼
Thread / Session
        │ one active task at a time
        ▼
Turn loop ──► Model provider (Responses API)
   │  ▲
   │  └──────── tool results / pending user input
   ▼
Tool router ──► approval policy ──► OS sandbox ──► process/files/MCP
   │
   └──────────► JSONL canonical history ──► SQLite projection
```

一次 turn 的最小循环是：组装历史与当前环境状态，流式调用模型，识别 tool call，经过审批和沙箱执行工具，把结果写回历史并继续采样；当模型只返回最终消息且没有后续输入时结束。官方源码中的入口已经迁移到 `codex-rs/core/src/session/turn.rs::run_turn`。

重要状态约束：

- 一个 session 同时最多运行一个 task；新输入可以中断或 steer 当前任务。
- 每个模型响应、tool call、tool result 和用户输入都进入有序历史。
- turn 结束会保留 response bookmark，用于继续或 fork。
- context compaction、异步 hooks、MCP 刷新和 pending input 都在 turn loop 边界协调。

## 关键分层

| 层 | 官方实现位置 | 我们应当学习/复用的内容 |
|---|---|---|
| CLI/TUI/非交互运行 | `codex-rs/cli`, `tui`, `exec` | 先保留非交互 CLI，TUI 后置 |
| 应用协议 | `app-server`, `app-server-protocol` | thread/turn/item、流式事件、审批回调、取消 |
| Agent 内核 | `core/src/session/turn.rs` | sampling loop、输入合并、取消、compaction |
| Tool 系统 | `core/src/tools`, `tools` | spec、registry、router、runtime 四层分离 |
| 执行与沙箱 | `exec-server`, `sandboxing` 相关 crates | 进程生命周期、文件权限、网络权限、平台适配 |
| 历史与状态 | `rollout`, `thread-store`, `state` | JSONL 作为事实源，SQLite 作为可重建索引 |
| 模型接入 | `client`, `codex-api`, `model-provider-info` | provider 配置、SSE、重试、用量、认证 |
| 扩展 | MCP、skills、hooks、plugins | MVP 后按真实需求逐项启用 |

## Tool 系统

`ToolRouter` 将模型可见的 schema 与实际 runtime 分离，并把 function call、custom tool call、tool search 等响应统一成内部 `ToolCall`。工具可以声明是否支持并行执行。这个边界值得直接采用，因为它允许同一内核同时支持：

- 内置文件、搜索、补丁和 shell 工具；
- MCP 工具；
- 延迟发现的工具；
- 后续的 code mode 和 subagent 工具。

我们第一版只需要 `read_file`、`search`、`apply_patch`、`exec` 四类能力。浏览器、MCP UI、多 agent 和实时语音都不应进入最初闭环。

## 权限与沙箱

Codex 把“模型能请求什么”“用户是否批准”“操作系统实际允许什么”作为三层处理：

1. Tool policy 决定工具是否暴露以及参数约束。
2. Approval policy 决定某次调用能否自动执行或需要用户决定。
3. OS sandbox 最终限制进程的文件系统和网络能力。

官方实现在 macOS 使用 Seatbelt，在 Linux 优先使用 bubblewrap，并保留 Landlock 兼容路径；Windows 使用专门的受限执行后端。这里最重要的设计原则是：审批只是交互层，安全边界必须由执行层落实。

## 持久化

官方实现把 rollout 写成追加式 JSONL，并将 SQLite 视为可重建的查询投影。这适合 agent 的事件流，因为：

- 崩溃后的恢复依据清晰；
- 可以完整重放模型输入、工具调用和结果；
- schema 演进时可从事实源重新建立索引；
- 调试时不依赖专用数据库工具。

我们 MVP 可以先只实现 JSONL，等 thread list、搜索和多会话性能需要出现后再加入 SQLite。

## App-server

官方 app-server 是面向 IDE、桌面端和 SDK 的双向接口。它使用类似 JSON-RPC 2.0 的消息结构，stdio 传输为逐行 JSON；线上的消息省略 `jsonrpc: "2.0"` 字段。顶层概念是 Thread、Turn、Item，并支持服务端主动发起审批或用户输入请求。

我们应当把协议作为内核外层的 adapter。第一版不必完整兼容所有方法，只实现：

- `initialize`
- `thread/start`、`thread/resume`
- `turn/start`、`turn/interrupt`
- item delta、tool start/completed、turn completed/failed
- approval request/response

## 模型 provider 限制

当前 Codex 官方实现只接受 Responses wire API；旧的 Chat Completions wire 被明确移除。这意味着直接在现有 Codex 内加入原生 Anthropic Messages API，不是简单增加一个 base URL。

可选方案有两个：

- 在 Codex 前面放一个 Responses-to-Messages adapter，先验证 Claude 模型在 Codex harness 中的行为。
- 在我们自己的内核定义模型无关事件，再分别实现 OpenAI Responses 和 Anthropic Messages adapter。

第二种是长期正确的结构；第一种适合作为短期实验。

## 第一轮源码阅读顺序

1. 协议和最小循环：`codex-rs/docs/protocol_v1.md`
2. turn loop：`codex-rs/core/src/session/turn.rs`
3. history/context：`codex-rs/core/src/session/mod.rs` 与 context manager
4. tools：`codex-rs/core/src/tools/router.rs`、`parallel.rs`
5. persistence：`codex-rs/rollout/src/recorder.rs`
6. external API：`codex-rs/app-server-protocol` 与 app-server
7. sandbox：core README 和各平台实现

## 一手来源

- [OpenAI Codex 主仓库](https://github.com/openai/codex)
- [OpenAI：Codex as a platform](https://developers.openai.com/blog/codex-as-a-platform)
- [Apache-2.0 LICENSE](https://github.com/openai/codex/blob/main/LICENSE)
- [Codex protocol v1](https://github.com/openai/codex/blob/main/codex-rs/docs/protocol_v1.md)
- [当前 turn loop](https://github.com/openai/codex/blob/main/codex-rs/core/src/session/turn.rs)
- [Tool router](https://github.com/openai/codex/blob/main/codex-rs/core/src/tools/router.rs)
- [Rollout recorder](https://github.com/openai/codex/blob/main/codex-rs/rollout/src/recorder.rs)
- [App-server protocol types](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/src/rpc.rs)
- [Core sandbox assumptions](https://github.com/openai/codex/blob/main/codex-rs/core/README.md)
- [Model provider wire API](https://github.com/openai/codex/blob/main/codex-rs/model-provider-info/src/lib.rs)
