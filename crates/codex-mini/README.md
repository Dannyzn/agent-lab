# Codex Mini

这是一个可运行的小型 coding-agent 对照实现。它采用官方 Codex 源码实验中确认的边界，现在作为 `PROTOCOL.md` 的 Python 可执行规格和 contract test runner；生产向实现位于 `rust/`。

## 已实现的闭环

```text
user prompt
    -> model adapter
    -> tool call
    -> policy + tool runtime
    -> function_call_output
    -> model adapter
    -> final message
```

内核输出 `thread.started`、`turn.started`、`item.started`、`item.completed`、`turn.completed` 和 `turn.failed` JSONL 事件。`exec_command`、`apply_patch`、`read_file`、`search` 通过同一个 registry 暴露给模型。

内核持有 canonical input history。每轮请求都包含此前的 `function_call` 和匹配的 `function_call_output`，与源码实验观察到的官方行为一致，也为后续 JSONL 恢复保留了明确边界。

## 运行离线测试

```bash
cd crates/codex-mini
PYTHONPATH=. python3 -m unittest discover -s tests -v
PYTHONPATH=. python3 examples/deterministic_demo.py
```

离线测试使用 `ScriptedClient`，因此不依赖网络和 API key。它覆盖“测试失败 -> 补丁 -> 测试通过 -> 最终消息”的完整多轮循环。

## 接入 OpenAI Responses API

```bash
cd crates/codex-mini
export OPENAI_API_KEY=...
PYTHONPATH=. python3 -m codex_mini \
  --workspace /absolute/path/to/project \
  --events /tmp/codex-mini-events.jsonl \
  "检查项目并修复失败测试"
```

默认命令白名单是 `python`、`python3`、`pytest`、`git`、`rg`、`ls`、`cat`。使用 `--allow-command NAME` 可以替换白名单。

## 当前安全边界

- 文件工具会解析真实路径，并拒绝 workspace 之外的路径。
- 命令执行不经过 shell，只接受 argv，并限制可执行文件名和超时。
- 这版还没有 OS 级沙箱。白名单进程仍可能主动访问 workspace 外的文件或网络，所以当前版本适合受控研究环境。
- 审批回调、取消、context compaction、恢复和真正的流式 SSE 留在后续迭代。
