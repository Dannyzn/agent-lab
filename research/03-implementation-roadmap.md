# 建议的实现路线

## 总体策略

构建一个公共内核，外加 `codex` 和 `claude` 两个 profile。profile 负责模型协议、默认工具命名、指令文件和用户体验；内核负责生命周期、工具调度、权限、持久化和事件流。

```text
                  ┌─ OpenAI Responses provider ─ Codex profile
CLI / app API ─── Agent kernel
                  └─ Anthropic Messages provider ─ Claude profile
                           │
          tools ─ permissions ─ sandbox ─ event store
```

推荐 Rust 作为内核语言。原因是 Codex 的可复用代码、跨平台进程控制和沙箱实现都在 Rust 生态内；长期运行的并发任务、取消和流式事件也适合强类型状态机。应用 UI 将来可以通过 JSONL/JSON-RPC 协议用 TypeScript 实现。

## Phase 0：可运行参考线

目标是确认我们能从源码构建官方 Codex，并采集一次最小任务的完整事件流。

交付物：

- 固定一个上游 commit，而不是长期跟随浮动的 `main`。
- 构建 `codex`，运行只读任务和一次文件修改任务。
- 记录模型请求、tool call、审批、sandbox、rollout 的对应关系。
- 用 app-server 的 schema 生成命令保存一份版本化协议快照。

退出条件：团队能从一条用户输入追踪到模型调用、工具执行、持久化和最终事件。

## Phase 1：最小 Codex-compatible kernel

建议的初始 crate 边界：

```text
crates/
  agent-protocol/       thread/turn/item/event 类型
  agent-kernel/         turn loop、取消、输入队列
  provider-openai/      Responses API + SSE
  agent-tools/          registry、router、tool runtime
  agent-exec/           shell 进程与超时
  agent-policy/         tool policy 与 approvals
  agent-store/          append-only JSONL
  agent-cli/            exec、resume、stream-json
```

第一版工具只有：读文件、搜索、补丁、执行命令。模型发出 tool call 后，系统必须能执行、记录结果、再次调用模型并得到最终消息。

MVP 验收：

- `agent exec "修复这个失败测试"` 能在临时 git repo 内完成多轮工具调用。
- 文件写入范围受 workspace root 限制。
- 命令可设置超时、取消，并完整收集 stdout/stderr/exit code。
- 高风险或越界调用会产生结构化 approval request。
- 进程中断后可以从 JSONL 恢复 session。
- `--output-format stream-json` 输出稳定的 thread/turn/item 事件。

## Phase 2：安全执行与产品可用性

- macOS Seatbelt 和 Linux bubblewrap adapter。
- 明确的 read/write/network capability model。
- context window 预算、tool output 截断和 compaction。
- git diff 展示、回滚点与变更摘要。
- app-server 的最小双向协议，支持 UI 回答 approvals。

## Phase 3：Claude-compatible profile

- `provider-anthropic`：Messages streaming、thinking/text/tool_use/tool_result 映射。
- `CLAUDE.md` 的发现、继承和优先级。
- Claude 风格工具名和权限模式。
- hooks 与 skills 的兼容格式。
- subagent、MCP、非交互 JSON 输出。

这里应共享 kernel 事件，不让 OpenAI 或 Anthropic 的 wire types 渗透到 tool runtime 和 store。

## 暂不进入 MVP 的功能

- 完整 TUI 和 IDE extension。
- 云端 task service、远程环境和账户同步。
- 多 agent 编排、实时语音、浏览器控制。
- plugin marketplace 和 MCP App UI。
- 与任一产品所有历史命令的逐项兼容。

## 下一次讨论需要拍板的事项

1. 是以官方 Codex fork 为长期产品，还是仅作参考、自己实现较小内核。
2. MVP 首个平台：只支持 macOS，还是同时支持 Linux。
3. 首个可用界面：非交互 CLI，还是一开始就做 TUI。
4. Claude profile 的目标：生态配置兼容，还是 CLI 命令也要求兼容。
5. 是否允许第一阶段使用 OpenAI/Anthropic 官方云 API；本地模型支持可以后置。
