# Agent Lab

这个工作区用于研究并实现一个可实际使用的本地 coding agent。目标不是复制两套代码，而是构建一个公共内核，并提供两种产品行为配置：

- `codex` profile：优先兼容 OpenAI Responses API、Codex 的 thread/turn/event 模型及审批与沙箱行为。
- `claude` profile：兼容 Anthropic Messages API，以及 Claude Code 的 `CLAUDE.md`、hooks、skills、subagents 和权限模式。

官方 Codex 构建与确定性端到端追踪已经完成。自有 `PROTOCOL.md`、共享 contract tests 和 Rust Phase 1 纵向闭环也已经完成；官方源码继续作为协议和行为参考，不作为 fork 基线。

## 目录

- `research/`：经过来源核验的架构与方案笔记。
- `decisions/`：需要我们共同确认的架构决策记录。
- `experiments/`：一次性验证程序和行为实验。
- `upstream/`：外部源码检出位置；不提交到本仓库。
- `contracts/`：跨语言确定性验收契约。
- `crates/codex-mini/`：Python 可执行规格与对照实现。
- `rust/`：当前生产向 Rust runtime。

## 当前结论

采用“官方实现作行为基线，自有小内核作产品起点”的路线。Rust runtime 目前由 8 个小 crate 组成，已经通过 Responses SSE、工具执行、补丁、复测、JSONL 和 Git diff 的完整离线端到端验收。Claude Code CLI 本体不是开源许可，因此 Claude profile 将采用公开文档与 API 的兼容实现，不复制其受限源码。

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
