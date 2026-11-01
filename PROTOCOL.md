# Agent Runtime Protocol v0.3

这份协议是我们自己的内核契约。它参考官方 Codex 的源码追踪结果，但不承诺与任一产品的全部外部接口兼容。

## 设计目标

- Provider wire type 不进入 kernel、tools 或 store。
- 同一 thread 的用户/助手消息与工具交互形成有序 canonical history。
- 每个 tool call 必须有且只有一个匹配的 tool result。
- 对外事件表达生命周期、审批、会话恢复与上下文压缩。
- JSONL 是追加式事实记录；读取端必须忽略未知字段。CLI 要求事件文件位于工具工作区外，避免工具篡改审计记录。

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

v0.1 / v0.2 的模型请求携带完整 history。v0.3 区分无损 canonical history 与预算约束下的 request history；后者可包含有界工具结果和确定性摘录。两者都不依赖 provider 侧的 `previous_response_id`。助手的中间文本与最终回答也进入 canonical history，以支持下一 turn。

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
  -> AwaitingApproval (when policy asks; returns to ExecutingTool on allow)
  -> Sampling
  -> Completed | Failed | Cancelled
```

v0.2 policy 返回 allow / ask / deny。审批只授予本次 call，不能扩大工作区、命令白名单、文件和网络能力。

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
| `turn.cancelled` | IDs |
| `approval.requested` | IDs, `approval_id`, `call_id`, `tool`, `arguments`, `reason`, `capabilities` |
| `approval.resolved` | IDs, `approval_id`, `decision` (`allow`, `deny`, `cancelled`), optional `error` |
| `session.resumed` | IDs |
| `session.recovered` | IDs, `unknown_tool_results`, `interrupted_sampling` |
| `context.compacted` | IDs, `before_tokens`, `after_tokens`, `summarized_pairs`（tokens 为本地估计） |

Item 类型：

- `command_execution`
- `file_change`
- `tool_call`
- `agent_message`

工具 item 从 `status=in_progress` 开始，以 `completed` 或 `failed` 结束。失败的工具调用仍然作为正常 `function_call_output` 返回模型，不自动令 turn 失败。

v0.2 取消中的工具以 `status=cancelled` 关闭 item。审批事件位于该工具的 `item.started` 与 `item.completed` 之间。

## Approval transport v0.2

- 默认 `--approval ask`：读和搜索自动允许，命令与补丁逐次审批。
- `--approval never`：需要审批的调用直接拒绝；不是自动执行模式。
- `--approval auto`：在宿主预设能力内自动允许；不绕过硬性 policy 或 OS sandbox。
- 人类模式在 stderr 展示工具、参数与能力，stdin 输入 `y`/`yes` 批准一次，其他输入或 EOF 拒绝。
- JSON 模式通过 stdout 事件发出申请，stdin 接收一行 `{"approval_id":"...","decision":"allow"}` 或 `deny`。ID 不匹配、非法 JSON、EOF 均拒绝。
- 默认审批时限 120 秒，可配置 1–600000 ms；超时或 handler 异常关闭该审批和 item，并终止 turn，防止遗留 stdin reader 读取后续申请。
- 审批拒绝是普通失败工具结果，匹配原 `call_id` 后继续供模型推理。

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
- v0.2 默认 Seatbelt（macOS）；不可用或启动失败时不自动退回无隔离执行。
- 文件能力为 `read-only` / `workspace-write`，命令网络能力为 `deny` / `allow`，默认可写工作区、禁网；Provider 的网络连接在宿主侧，不受工具禁网规则限制。
- 工具进程可读工作区与明确的系统运行时目录，仅可写工作区（可写模式）和本次独立临时目录。Git 元数据禁止工具写入，文件工具禁止直接访问 `.git`。
- 子进程清空继承环境；只注入固定 PATH、独立 HOME/TMPDIR、语言和安全 Git/Python 设置，不传递 API key。
- `--sandbox none` 必须由宿主显式选择并产生 stderr 警告；此时子进程文件和网络能力不再由 OS 强制执行。

## Cancellation v0.2

- sticky cancellation token 用于模型请求（含重试等待）、审批和命令。
- CLI 接收 SIGINT/SIGTERM 后取消；正常取消退出码 130。
- 命令在独立 POSIX process group 中运行；取消、超时和正常父进程退出后杀死该 group，并等待直接子进程退出。
- 命令输出边读取边截断，防止大输出无限累积；清理后管道最多再等待 1 秒（每个流），超时明确报错。
- 取消审批发出 `approval.resolved(decision=cancelled)`；关闭已开始的工具 item 后，以 `turn.cancelled` 结束，不发出 `turn.failed` 或 `turn.completed`。
- 同步文件操作在工具边界观测取消；已完成的文件改动不会自动回滚。
- 当前清理保证常规子孙进程的同组清理；恶意 `setsid`/进程组逃逸不在保证范围内，仍需要后续更强进程监管。

## Durable sessions v0.3

`--session PATH` 创建新 WAL；`--resume PATH` 重放 WAL。会话文件和事件文件分开，均位于工具工作区外。WAL schema `version=1`，外层为：

```json
{"version":1,"seq":0,"record":{"kind":"created","data":{"thread_id":"...","workspace":"...","capabilities":{},"context_policy":{"max_tokens":32768,"reserve_output_tokens":4096,"tool_output_bytes":8192,"keep_recent_pairs":2}}}}
```

`capabilities` 在实际记录中包含工具工作区/沙箱/白名单、审批模式及 `max_steps`。路径私有、创建不覆盖、0600、单写者锁。每条记录写入并同步后再继续；状态转换先验证。只允许截断最后一条未换行的半写数据，完整坏记录、版本不支持或 seq 不连续必须失败。

| Record kind | 作用 |
|---|---|
| `created` | thread、工作区、能力与上下文策略 |
| `turn_started` | 新 turn ID 与完整用户输入 |
| `sampling_started` / `sampling_interrupted` | 持久化模型调用次数及中断状态 |
| `model_returned` | 完整中立响应、待执行调用与最终消息 ID |
| `tool_started` | 执行意图；审批通过后、工具实际执行前持久化 |
| `tool_finished` | 完整工具返回和模型投影；先落盘再发 item 完成事件 |
| `context_compacted` | 投影视图、源 canonical 项数与估算量 |
| `turn_finished` | completed / failed / cancelled 与错误 |
| `event` | 先保存完整公开事件，再投递普通 event sink |

恢复规则：

- Running turn 不接受新 prompt；恢复相同 thread/turn，保留模型已消耗步数与原上限。
- 完成后不带 prompt 返回缓存答案，不发模型请求；有 prompt 则在同 thread 开新 turn。
- Failed / Cancelled 后不带 prompt 在同 thread 开新 turn，加入检查现状后续做的消息；有 prompt 使用新输入。没有自动回滚。
- 旧审批一律关闭，未执行的调用必须重新评估 policy 并获得新审批。
- 已有 `tool_started` 而没有结果时，追加失败结果 `execution_state=unknown`，不自动重跑。同批后续调用补 `execution_state=not_executed`；unknown 产生的恢复屏障在再次崩溃后也保留，直到下一次模型采样。
- 已有工具结果不再执行；若缺 item 完成事件则补齐。最终消息已保存时不再采样；已有终止状态但缺终止事件时补齐。
- 工作区、沙箱、审批、白名单、步数上限及上下文策略不一致时拒绝。CLI 省略 workspace、步数/上下文参数时使用已存值；能力开关仍由宿主显式匹配。

持久化事件包含 `event_id`。WAL 是权威记录，普通 stdout / `--events` 没有事务投递保证：进程可能在记录事件后、投递事件前退出。宿主需要补事件时可读取 WAL 的 `event` 记录，按 ID 去重；本版没有消费游标或自动重发所有历史事件。会话控制事件可出现在已结束 turn 的终止事件之后，不代表该 turn 重新执行。

日志保存任务、源码和工具返回；不保存连接配置或鉴权头。宿主恢复时仍需提供模型连接信息，内核不验证两次连接是否为同一模型。日志既未加密也无防篡改认证；不可提交为普通项目源码。

## Context budgets v0.3

- 默认本地预算 32768，输出预留 4096，单个工具投影最多 8192 bytes；默认优先保留最近两组工具交互。
- 估算量为 history 与 tools 的 JSON UTF-8 字节数，加 1024 框架余量和输出预留；不是精确 tokenizer，也不保证覆盖网关隐藏 prompt。CLI 将输出预留同时设置为 provider 的 `max_output_tokens`。
- 完整工具返回保留在 canonical/WAL；发给模型的结果可保留状态、退出码及 Unicode 安全的首尾摘录，并明确标记截断。工具执行层原有 65536-byte 等输出限制仍生效。
- 超限时从 canonical 重建视图：用户消息逐字保留，旧工具只按完整 call/output 对移除，历史事实采用有界摘录，不作为 system 消息。可逐步减少保留的最近工具对，直到估算满足预算。
- 压缩记录和原始历史同时存在；resume 恢复原投影视图。压缩后仍超限，在 API 请求前失败。历史摘要会损失细节，无无限长任务保证。

## Turn invariants

1. 新 thread 的首个事件是 `thread.started`；resume 不重复发它。
2. `turn.started` 在任何 item 事件之前。
3. 每个 tool item 恰好有一个 started 和一个 completed。
4. agent message 只产生 completed。
5. 成功 turn 的执行生命周期以 `turn.completed` 结束；后续会话控制事件不重新打开 turn。
6. 未处理运行时异常以 `turn.failed` 结束；日志/事件写入本身失败时须中止，不能伪造已持久化的终止。
7. 达到 `max_steps` 必须失败，不能静默结束。
8. 一个 turn 只有一种终止事件：completed、failed 或 cancelled。
9. 每个已请求的 approval 恰有一次 resolved，批准不得复用于其他 call。

## Contract fixture

`contracts/fix-failing-test/contract.json` 是 v0.1 的首个语言无关验收用例。实现必须从相同初始文件和 scripted model response 得到相同的规范化事件类型、最终消息、文件内容和测试结果。
