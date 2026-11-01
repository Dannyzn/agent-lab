# Agent Lab

这个工作区用于研究并实现一个可实际使用的本地 coding agent。目标不是复制两套代码，而是构建一个公共内核，并提供两种产品行为配置：

- `codex` profile：优先兼容 OpenAI Responses API、Codex 的 thread/turn/event 模型及审批与沙箱行为。
- `claude` profile：兼容 Anthropic Messages API，以及 Claude Code 的 `CLAUDE.md`、hooks、skills、subagents 和权限模式。

官方 Codex 构建与确定性端到端追踪已经完成。自有 `PROTOCOL.md`、共享 contract tests、Rust Phase 1 纵向闭环及 macOS Phase 2 审批 / Seatbelt / 取消闭环已经完成；官方源码继续作为协议和行为参考，不作为 fork 基线。

## 目录

- `research/`：经过来源核验的架构与方案笔记。
- `decisions/`：需要我们共同确认的架构决策记录。
- `experiments/`：一次性验证程序和行为实验。
- `upstream/`：外部源码检出位置；不提交到本仓库。
- `contracts/`：跨语言确定性验收契约。
- `crates/codex-mini/`：Python 可执行规格与对照实现。
- `rust/`：当前生产向 Rust runtime。

## 当前结论

采用“官方实现作行为基线，自有小内核作产品起点”的路线。Rust runtime 目前由 9 个小 crate 组成，已经通过 Responses SSE、工具执行、补丁、复测、JSONL 和 Git diff 的完整离线端到端验收。Claude Code CLI 本体不是开源许可，因此 Claude profile 将采用公开文档与 API 的兼容实现，不复制其受限源码。

继续讨论前请先看：

1. [Codex 架构调研](research/01-codex-architecture.md)
2. [Claude Code 开源边界](research/02-claude-code-boundary.md)
3. [建议的实现路线](research/03-implementation-roadmap.md)
4. [待确认的首个架构决策](decisions/0001-codex-first.md)
5. [官方源码构建与追踪结果](experiments/codex-source-build/RESULT.md)
6. [Mini 内核边界](research/04-mini-kernel-boundary.md)
7. [自有运行时协议](PROTOCOL.md)
8. [Rust Phase 1 结果](experiments/rust-phase1/RESULT.md)
9. [从官方源码实验到 Rust 最小运行时的学习报告](research/05-codex-mini-learning-report.md)

## Phase 2 当前状态（2026-11-01）

默认逐次审批命令和补丁，子进程运行在 macOS Seatbelt 中，可写工作区但禁止联网。支持只读能力、JSON 审批回传、SIGINT/SIGTERM 取消以及进程组清理。`--approval auto` 不会绕过沙箱；`--sandbox none` 是显式的开发选项。

- [Phase 2 实验与验收](experiments/phase2/README.md)
- [Phase 2 结果和限制](experiments/phase2/RESULT.md)
- [如何判断 agent 可用：测试指南](research/06-agent-acceptance.md)
- [单文件修复与多文件任务验收](experiments/agent-eval/RESULT.md)

Phase 2 验证的是运行时与边界。另已完成真实模型的单文件修复和三文件功能任务验收：外部测试通过、保护文件未变，且无被拒绝的审批请求；样本量仍不足以推断通用任务可靠性。具体供应商配置、模型标识、网关地址和原始实测记录仅保留在本地，不纳入版本控制。恢复、compaction 和 Anthropic profile 仍为后续阶段。
