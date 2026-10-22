# Agent Runtime Protocol v0.1

这份协议是我们自己的内核契约。它参考官方 Codex 的源码追踪结果，但不承诺与任一产品的全部外部接口兼容。

## 设计目标

- Provider wire type 不进入 kernel、tools 或 store。
- 一次 turn 的所有模型输入形成有序 canonical history。
- 每个 tool call 必须有且只有一个匹配的 tool result。
- 对外事件只表达 thread、turn 和 item 生命周期。
- JSONL 是追加式事实记录；读取端必须忽略未知字段。

## Canonical model history

第一项是用户消息：

```json
{"type":"message","role":"user","content":[{"type":"input_text","text":"修复失败测试"}]}
```

模型请求工具后，kernel 依次追加：

```json
{"type":"function_call","call_id":"call-1","name":"exec_command","arguments":"{\"argv\":[\"python3\",\"-m\",\"unittest\"]}"}
{"type":"function_call_output","call_id":"call-1","output":"{\"success\":false,\"exit_code\":1,\"output\":\"...\"}"}
```

下一次模型请求必须携带此前的完整 history。v0.1 不依赖 provider 侧的 `previous_response_id`。

## Model response

Provider adapter 将厂商响应归一化为：

```json
{
  "response_id": "response-1",
  "text": null,
  "tool_calls": [
    {"call_id":"call-1","name":"exec_command","arguments":{"argv":["python3","-m","unittest"]}}
  ]
}
```

一次响应可以包含多个 tool call。若 `tool_calls` 非空，kernel 先执行工具并继续采样；若为空，则 `text` 必须存在并结束 turn。

## Runtime state machine

```text
Idle
  -> Sampling
  -> ExecutingTool
  -> Sampling
  -> Completed | Failed | Cancelled
```

`AwaitingApproval` 保留为 v0.2 状态。v0.1 的 policy 只能 allow 或 deny。

## Public events

所有事件包含 `type` 和 UTC `timestamp`。相关事件还包含 `thread_id`、`turn_id`。

| Event | Required payload |
|---|---|
| `thread.started` | `thread_id` |
| `turn.started` | `thread_id`, `turn_id` |
| `item.started` | IDs, `item` |
| `item.completed` | IDs, `item` |
| `turn.completed` | IDs |
| `turn.failed` | IDs, `error` |

Item 类型：

- `command_execution`
- `file_change`
- `tool_call`
- `agent_message`

工具 item 从 `status=in_progress` 开始，以 `completed` 或 `failed` 结束。失败的工具调用仍然作为正常 `function_call_output` 返回模型，不自动令 turn 失败。

## Tools v0.1

### `read_file`

```json
{"path":"relative/path.txt"}
```

### `search`

```json
{"query":"literal text","path":"."}
```

### `apply_patch`

```json
{"patch":"*** Begin Patch\n*** Update File: file.py\n@@\n-old\n+new\n*** End Patch"}
```

v0.1 只支持更新已存在的 UTF-8 文件。

### `exec_command`

```json
{"argv":["python3","-m","unittest"],"timeout_ms":30000}
```

命令不经过 shell。`argv[0]` 必须是 policy 允许的裸可执行文件名。

## Workspace and execution rules

- 文件路径以 canonical workspace root 为基准。
- `..`、绝对路径和指向 workspace 外部的符号链接必须拒绝。
- 命令超时范围为 1–120000 ms。
- stdout 与 stderr 合并返回，v0.1 最多保留 65536 bytes。
- v0.1 的路径 policy 不是 OS sandbox；子进程仍可能自行访问外部文件或网络。

## Turn invariants

1. `thread.started` 是第一条事件。
2. `turn.started` 在任何 item 事件之前。
3. 每个 tool item 恰好有一个 started 和一个 completed。
4. agent message 只产生 completed。
5. 成功 turn 的最后一条事件是 `turn.completed`。
6. 未处理异常的最后一条事件是 `turn.failed`。
7. 达到 `max_steps` 必须失败，不能静默结束。

## Contract fixture

`contracts/fix-failing-test/contract.json` 是 v0.1 的首个语言无关验收用例。实现必须从相同初始文件和 scripted model response 得到相同的规范化事件类型、最终消息、文件内容和测试结果。
