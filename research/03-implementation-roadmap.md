# 实现路线与当前阶段

更新：2026-11-01。早期 Phase 1 草案中的审批、取消与恢复已按实际依赖拆到 Phase 2 和 Phase 3；以本路线及各阶段 RESULT 为准。

## 总体策略

构建公共 Rust 内核，以 Provider adapter 和 profile 接入 OpenAI / Anthropic。模型 wire types 不进入工具运行时和存储。官方 Codex 作为固定版本的可执行参考，不作为 fork 基线。

## Phase 0：官方行为基线（已完成）

- 固定上游 commit 并完成源码构建。
- 真实官方二进制接本地确定性 Responses 服务，完成修复/复测流程。
- 跟踪 canonical history、function call/output 和 thread/turn/item 事件。
- 记录 schema、源码规模、构建结果与网络预检边界。

证据：`experiments/codex-source-build/RESULT.md`。

## Phase 1：最小纵向闭环（已完成）

- `PROTOCOL.md` 与语言无关 contract。
- Python 可执行规格和 Rust workspace。
- Responses HTTP/SSE、4 个工具、完整 history、JSONL、CLI 和 Git diff。
- 同一 contract 通过 Python、Rust kernel 和真实 Rust CLI 三层验收。

证据：`experiments/rust-phase1/RESULT.md`。

## Phase 2：审批、OS 沙箱与取消（macOS 已实现并验收）

- 每次调用的 allow / ask / deny，默认命令与补丁询问。
- 人类审批和 JSON 双向审批、超时拒绝与唯一 ID 关联。
- macOS Seatbelt，文件与网络能力、最小环境、工作区与独立临时目录。
- SIGINT/SIGTERM、sticky cancellation、命令进程组清理及有界输出。
- 自动验收批准/拒绝、只读、越界、禁网以及不同等待状态下取消。

证据：`experiments/phase2/RESULT.md`。当前范围是受人监督的 macOS 使用；Linux、恶意进程组逃逸和对抗性工作区访问仍需后续加强。

## Phase 3：持久化与长任务（macOS CLI 已实现并确定性验收）

- 保存完整 canonical rollout，而不只保存公开生命周期事件。
- resume、crash recovery、turn 恢复和一致性检查。
- token budget、tool output 预算与上下文 compaction。
- 恢复后仍保持 call/result 的一一对应。

证据：`experiments/phase3/RESULT.md`。预算为本地保守估计，压缩为确定性摘录；SIGKILL 不保证子进程清理。已补首轮真实模型的恢复、同会话追加需求和长规格压缩评估，保留失败与对照记录，详见 `experiments/session-eval/RESULT.md`。v0.3.1 改进跨文件证据摘录与续做提示；仍需扩大样本和重复次数。

## Phase 4：Claude-compatible profile（待做）

- Anthropic Messages streaming 与中立类型映射。
- `CLAUDE.md` 发现、继承和优先级。
- Claude 风格工具命名、权限模式及 hooks / skills。
- MCP 和 subagent 能力。

## 产品验收与后续平台

在扩展功能的同时，按 `research/06-agent-acceptance.md` 进行真实模型冒烟和多任务评估。当前确定性测试不代表已经测得云端模型成功率。

Linux sandbox、TUI / IDE、云端执行、多 agent 编排和账户同步均不属于已完成范围。先以 CLI 和稳定 JSONL 协议支持受监督使用，再根据真实任务失败样本确定优先级。
