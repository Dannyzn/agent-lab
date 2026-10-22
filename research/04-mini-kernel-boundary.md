# Codex Mini 内核边界

这个小内核根据官方 Codex 构建与事件追踪实验建立，不复制官方工程结构。Python 版本现在作为可执行规格；生产向 Rust workspace 已完成 Phase 1 纵向闭环。

## 采用的设计

| 官方边界 | Mini 对应实现 | 原因 |
|---|---|---|
| Thread / Turn / Item | JSONL 事件类型 | 保留可追踪的外部生命周期 |
| Model provider | `ModelClient` protocol | OpenAI 与未来 Anthropic wire type 不进入内核 |
| Tool spec / router / runtime | `ToolRegistry` | schema 和执行共用稳定工具名 |
| Approval / sandbox boundary | `WorkspacePolicy` | 先落实路径、argv、命令和超时约束 |
| Rollout JSONL | `EventSink` | 追加事实源便于调试和重放 |

## 有意省略的能力

- TUI、IDE 与 app-server 完整 JSON-RPC surface
- MCP、skills、hooks、plugins 和多 agent
- SQLite 投影、会话恢复和 context compaction
- Seatbelt、bubblewrap、Landlock 与 Windows sandbox
- WebSocket/SSE 增量显示、重试、速率限制和 token accounting

这些能力不会进入最初 turn loop。后续只有当验收场景需要时，才在协议或 adapter 层增加。

## 下一步增量

1. 把策略结果扩成 allow / ask / deny，并通过事件请求审批。
2. 给 Rust kernel 增加 cancellation token 和进程树清理。
3. 接入 macOS Seatbelt executor，让安全边界落到 OS。
4. 将 `EventSink` 扩展成可重放的 canonical rollout history。
5. 实现 resume、compaction 和 crash recovery。
6. 用同一组 provider-neutral contract tests 接 Anthropic Messages adapter。
