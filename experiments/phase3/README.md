# Phase 3：持久化、恢复与上下文预算

前提：支持 Seatbelt 的 macOS、已构建 `target/debug/our-agent`。测试使用真实 Rust CLI 和回环 Responses SSE 服务，不需要云服务、真实模型或 API key。

```bash
cargo build --workspace --locked
cargo test --workspace --locked
./experiments/phase3/run-contract.sh
```

测试会主动 SIGKILL 宿主进程。命令崩溃用例使用会自行退出的短任务，并等待其退出后再恢复；它不把“未重复执行”误称为“SIGKILL 自动清理子进程”。所有 workspace、WAL、HTTP 请求、stdout 和 stderr 保存在被 Git 忽略的 `artifacts/`，每次使用独立目录。

## 手工使用

在宿主终端中设置 `OPENAI_API_KEY`，以 `AGENT_TEST_MODEL` 指定账户可用模型。会话文件和普通事件文件必须使用不同路径，并位于工具工作区外。

```bash
./target/debug/our-agent \
  --workspace /absolute/path/to/disposable-project \
  --session /tmp/agent-task.session.jsonl \
  --model "$AGENT_TEST_MODEL" \
  '检查项目，修复失败测试并复测'

# 进程中断后，不带新任务恢复当前 turn。
./target/debug/our-agent \
  --resume /tmp/agent-task.session.jsonl \
  --model "$AGENT_TEST_MODEL"

# 当前 turn 已完成后，在同一 thread 上开始新 turn。
./target/debug/our-agent \
  --resume /tmp/agent-task.session.jsonl \
  --model "$AGENT_TEST_MODEL" \
  '基于上次结果继续完善错误处理'
```

恢复时可省略 workspace，它从会话读取。原来显式设置的 approval / sandbox / filesystem / network / allow-command 必须保持一致；不会自动恢复更宽的能力。上下文预算、输出预算和步数上限省略时沿用会话配置。模型与连接配置由宿主重新提供，当前 CLI 即便只读取已完成回答也仍要求环境中有 API key，但不会为此发送模型请求。

| 会话状态 | 不带 prompt 恢复 | 带 prompt 恢复 |
|---|---|---|
| Running | 恢复原 turn；关闭旧审批；按日志处理待执行调用 | 拒绝，避免把新任务混入未完成 turn |
| Completed | 返回缓存回答，不采样 | 同一 thread 新 turn，保留原历史 |
| Cancelled / Failed | 同一 thread 新 turn，追加先检查工作区的续做提示 | 同一 thread 新 turn，使用新任务 |

`--context-tokens` 默认 32768，`--max-output-tokens` 默认 4096（同时发给 provider），`--tool-output-bytes` 默认 8192。最近优先保留两组工具交互；必要时减少到零组，仍超限则在 API 请求前失败。所有用户消息都保留，因此无限长对话不在保证范围内。

恢复与长任务的边界见 [设计决策](../../decisions/0003-durable-session-and-context.md)，验收结果见 [RESULT](RESULT.md)。
