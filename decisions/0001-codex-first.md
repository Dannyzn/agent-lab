# ADR-0001：从 Codex 开始建立基线

状态：已接受（2026-10-22）。

## 背景

目标是实现一个具有 Codex 与 Claude Code 两种兼容体验的可用 coding agent。Codex 的核心实现采用 Apache-2.0 开源；Claude Code CLI 本体受 Anthropic 商业条款约束。

## 提议

先从 Codex 开始：

1. 构建并运行固定版本的官方 Codex，形成测试与行为基线。
2. 以其公开协议和源码为参考，先用 Python 原型固定自己的最小 kernel，再按长期需要决定是否迁移 Rust。
3. 内核从一开始就定义模型无关的事件，避免绑定 Responses API。
4. Codex profile 稳定后，实现 Anthropic Messages provider 和 Claude profile。

## 理由

- 可以用真实源码验证 agent loop、审批、sandbox 和持久化，而不是从产品表象猜测。
- 先获得可运行参考物，能建立回归测试和行为差异清单。
- 公共内核降低双产品路线的维护成本。
- Claude Code 的许可边界要求我们独立实现其 harness 行为。

## 决定

选择方案 B：Codex 只作参考，从较小的自有 kernel 开始。

Phase 0 已经构建固定 commit 并完成源码版工具循环追踪。观察到上游包含 159 个 workspace member、超过 211 万行 Rust，debug target 约 16 GB；完整 fork 会把大量当前不需要的 UI、云任务、扩展、MCP 与平台适配带入维护范围。`crates/codex-mini` 因此只保留 provider、turn loop、tool、policy 和 JSONL event 五个边界。
