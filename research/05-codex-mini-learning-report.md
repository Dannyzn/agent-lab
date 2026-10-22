# Codex Mini：从官方源码实验到 Rust 最小运行时的实现学习报告

日期：2026-10-22
项目目录：`agent-lab/`

这份报告回答三个问题：我们具体做了什么，实验让我们确认了哪些事实，以及现在的 Codex Mini 到底是什么。

先给结论：我们已经完成了一个“生产方向的第一阶段纵向闭环”。它不是完整 Codex，也不是官方仓库的裁剪版；它是一个从零实现的、约 1,300 行 Rust 的最小 coding-agent 运行时。它能通过 Responses 兼容接口调用模型，维护完整对话历史，执行命令和文件工具，修改代码，复跑测试，输出事件流和 Git diff。与此同时，Python 版本被保留为可执行规格，官方 Codex 源码被保留为行为参考和协议验证器。

当前实现已经证明核心 agent loop 可以独立成立，但还没有加入操作审批、OS 级沙箱、恢复与压缩等生产安全能力，因此适合继续研发和受控实验，还不适合无人值守地运行在任意仓库中。

## 1. 最终产出了什么

整个工作分成三层，每层解决不同问题。

| 层次 | 产物 | 作用 |
|---|---|---|
| 官方行为基线 | 官方 Codex 源码构建与端到端追踪 | 回答“真实 Codex 怎样组织请求、工具调用和事件” |
| 可执行规格 | Python `codex-mini` + 共享 contract | 用最小代码快速固定概念和预期行为 |
| 生产方向内核 | 8 个 Rust crate + `our-agent` CLI | 实现真实 HTTP/SSE、工具执行、事件记录和完整修复闭环 |

关键文件如下：

- `PROTOCOL.md`：我们自己的运行时协议 v0.1。
- `contracts/fix-failing-test/contract.json`：语言无关的确定性验收契约。
- `crates/codex-mini/`：Python 可执行规格。
- `rust/`：生产方向的 Rust 工作区源码。
- `experiments/codex-source-build/RESULT.md`：官方源码构建和追踪证据。
- `experiments/rust-phase1/RESULT.md`：Rust 第一阶段结果。
- `experiments/rust-phase1/run-contract.sh`：完整 HTTP/SSE 端到端实验入口。

这里最需要澄清的是“哪个才是现在的 Codex Mini”：

- Python 版本是参考实现，用来快速表达协议、生成预期事件并跑 contract tests。
- Rust 版本才是当前准备继续演进的最小运行时内核。
- 官方 Codex 不在我们的产品代码依赖链上，它负责提供事实和边界，而不是成为 fork 基线。

## 2. 为什么先构建一次官方 Codex

如果目标只是做一个能循环调用工具的 demo，确实不需要先编译官方源码。但我们的目标是做一个以后可扩展、行为可解释的 coding agent，因此需要先验证几个容易凭感觉做错的地方：

1. 模型每次收到的是增量上下文，还是完整历史？
2. 工具失败会直接终止 turn，还是作为普通结果继续交给模型？
3. 内部模型消息如何映射为对外稳定事件？
4. Provider 协议、agent kernel、工具执行和安全策略之间的边界在哪里？
5. 官方仓库的规模是否适合作为我们直接 fork 的起点？

### 2.1 构建结果

我们固定了官方源码的精确版本，避免调研过程中上游变化导致结论漂移：

- commit：`d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`
- 源码归档 SHA-256：`e3e1af2a06ec9c1bd8c510f80d520f932446e440bf58a1799c8375765f7c4316`
- Rust：1.99.1
- Cargo：1.99.1
- 构建出的 CLI：`codex-cli 0.0.0`

首次完成编译耗时 9 分 33 秒；增量构建耗时 2.56 秒。debug 二进制约 1.2 GB，整个 target 目录约 16 GB。

源码规模也给出了明确答案：该快照有 159 个 workspace member、5,409 个 Rust 文件、约 211.7 万行 Rust；仅 core 就有 595 个文件、约 25.7 万行。它非常适合当参考实现，但直接 fork 会把大量首阶段不需要的产品功能、兼容逻辑和基础设施一起带进来。

由于当时环境中的 GitHub Git transport 不稳定，我们把 8 个精确 revision 的 Git 依赖下载成源码快照，并改写为本地 path dependency。依赖的名称、版本和 revision 仍然记录在实验材料中，所以构建仍然可审计。

### 2.2 官方端到端事件追踪

我们没有止步于“编译成功”，而是给官方二进制接了一个本地确定性的 Responses API，让它修复一个真实的失败测试。模型端按脚本依次给出四个响应：

1. 调用 `exec_command` 跑测试；
2. 看到退出码 1 后调用 `apply_patch`；
3. 再次调用 `exec_command` 复测；
4. 看到退出码 0 后返回最终回答。

实际结果是：修改前 2 个测试失败，修改后 2 个测试通过；补丁只有一行，把 `left - right` 改为 `left + right`。

官方 CLI 对外发出了 10 个 JSONL 事件：

```text
thread.started
turn.started
item.started    command_execution
item.completed  command_execution (failed)
item.started    file_change
item.completed  file_change (completed)
item.started    command_execution
item.completed  command_execution (completed)
item.completed  agent_message
turn.completed
```

这组事件有两个关键含义。

第一，命令执行失败不等于 turn 失败。第一次测试失败是 agent 获取信息的正常步骤，失败结果会作为 `function_call_output` 返回模型，模型据此决定下一步。只有运行时本身出现未处理异常、超过步数等情况，turn 才应该进入 `turn.failed`。

第二，模型协议和用户可见事件不是同一种数据。模型需要完整而精确的工具调用历史；用户或 UI 更适合消费稳定的 thread、turn、item 生命周期。把两者混成一个结构，后续很难同时兼容 Provider 变化和产品展示需求。

### 2.3 Canonical history 的实证

追踪共捕获 4 次 Responses 请求。它们携带的 history 长度分别是：

```text
1 -> 3 -> 5 -> 7
```

最初只有一条用户消息。每完成一个工具调用，历史增加两项：

```json
{"type":"function_call","call_id":"call-1","name":"exec_command","arguments":"..."}
{"type":"function_call_output","call_id":"call-1","output":"..."}
```

下一次请求会重新发送此前的完整 canonical history。该次官方实验没有使用 `previous_response_id` 串接状态。这促使我们在 v0.1 中让 kernel 自己拥有完整历史，避免把正确性依赖在某个 Provider 的服务端状态上。

官方追踪中模型可见工具共有 10 个，而公开事件只保留稳定的生命周期信息。这再次支持“Provider、kernel、tools、events 分层”的设计。

### 2.4 实时 Provider 预检的边界

官方二进制还做了真实 Provider 预检：它成功进入 thread 和 turn，WebSocket 重试从 2/5 到 5/5，随后回退到 HTTPS，但当前运行环境连接 `chatgpt.com` 超时。因此我们不能把这部分记录称为成功的云端推理。

本地确定性 E2E 已经覆盖真实官方二进制、Responses 请求结构、工具循环和事件映射；它排除了外部网络这一不稳定变量。后续有可用凭据和网络时，仍应再做一次真实云端冒烟测试。

## 3. 如何把观察固定成我们自己的协议

单靠研究笔记无法防止实现慢慢偏离，所以我们把可观察到的行为收敛成了 `PROTOCOL.md`，再把其中最重要的一条流程写成语言无关 contract。

### 3.1 协议的五个核心边界

`PROTOCOL.md` v0.1 固定了五条原则：

1. Provider 的 wire type 不能进入 kernel、tools 或 store。
2. 一个 turn 的全部模型输入由 kernel 维护为有序 canonical history。
3. 每个 tool call 必须有且只有一个对应 tool result。
4. 对外事件表达 thread、turn 和 item 生命周期。
5. JSONL 是追加式事实记录；读取端必须忽略未知字段，以支持兼容扩展。

Provider adapter 只需要把厂商响应归一化为：

```text
ModelResponse
├── response_id
├── text: optional
└── tool_calls[]
    ├── call_id
    ├── name
    └── arguments
```

如果 `tool_calls` 非空，kernel 执行工具、追加结果并继续采样；如果为空，则 `text` 必须存在，turn 才能完成。

### 3.2 状态机

v0.1 的状态机是：

```text
Idle
  -> Sampling
  -> ExecutingTool
  -> Sampling
  -> Completed | Failed | Cancelled
```

`AwaitingApproval` 已经在协议中预留，但要到 v0.2 才实现。现在的 policy 只能直接 allow 或 deny。

### 3.3 四个最小工具

我们只保留完成修复闭环真正需要的工具：

| 工具 | 输入 | 作用 |
|---|---|---|
| `read_file` | 相对路径 | 读取工作区内 UTF-8 文件 |
| `search` | 查询文本、相对路径 | 递归进行字面量搜索 |
| `apply_patch` | patch 文本 | 精确修改已存在的 UTF-8 文件 |
| `exec_command` | argv、超时 | 不经 shell 直接启动进程 |

当前 `apply_patch` 是有意收窄的子集，只支持 `*** Update File` 和精确 hunk 匹配。它暂时不负责新建、删除、重命名或二进制文件。

命令使用 argv 数组而不是 shell 字符串，这样可以减少转义歧义，也让 allowlist 检查针对明确的可执行文件生效。超时范围是 1 到 120,000 毫秒，stdout/stderr 合并后最多保留 65,536 字节。

### 3.4 确定性 contract

共享 contract 位于 `contracts/fix-failing-test/contract.json`。它同时描述：

- 初始工作区文件；
- 用户 prompt；
- 4 个 scripted model response；
- 预期请求次数和 history 长度；
- 预期事件类型和 item 类型；
- 预期最终消息和文件内容。

这个 contract 的价值不只是测试一个加法函数。它验证的是完整因果链：

```text
失败测试
  -> 模型看到工具结果
  -> 模型请求补丁
  -> 文件真的改变
  -> 模型请求复测
  -> 测试通过
  -> 模型结束 turn
```

Python 和 Rust 都读取同一份 fixture，因此以后重构任一实现时，不需要靠人工判断“看起来差不多”。

## 4. Python 原型具体做了什么

Python 版本位于 `crates/codex-mini/`，只依赖 Python 3.8+ 标准库。它的职责是用最短路径验证抽象，而不是作为最终生产运行时。

| 模块 | 职责 |
|---|---|
| `agent.py` | 执行 turn loop，维护 history，调度模型和工具 |
| `protocol.py` | 定义 `ModelClient`、`ModelResponse`、`ToolCall` |
| `providers.py` | 提供 OpenAI Responses adapter 和 scripted provider |
| `tools.py` | 实现读取、搜索、补丁和命令工具 |
| `policy.py` | 约束工作区路径和可执行命令 |
| `events.py` | 追加写入 JSONL 事件 |

Python 层通过了 3 个测试：共享修复 contract、工作区/命令/符号链接边界、OpenAI 请求的完整 history 映射。

这一步让我们在进入 Rust 之前确认了接口形状、事件顺序和安全边界。它也保留为独立 oracle：如果以后 Rust 重构后行为异常，可以用同一 contract 对比两种实现。

## 5. Rust 最小运行时的架构

Rust workspace 由 8 个 crate 组成，共 8 个 `.rs` 文件、1,297 行 Rust。它不是把所有代码塞进一个二进制，而是按照变化原因拆开边界：

```text
agent-cli
   |
   v
agent-kernel ---------> agent-protocol
   |                          ^
   +----> agent-tools --------+
   |         |                |
   |         +-> agent-policy |
   |         +-> agent-exec   |
   |                          |
   +----> agent-store --------+
   |
   +----> provider-openai ----+
```

### 5.1 `agent-protocol`

这是最小公共语言，定义 `ToolCall`、`ModelResponse`、异步 `ModelClient` 和 `EventSink`。事件辅助函数统一补充事件类型和 UTC RFC 3339 时间戳。

这个 crate 不知道 OpenAI SSE、文件系统或 CLI。以后添加 Anthropic provider 时，kernel 可以继续消费相同的 `ModelResponse`。

### 5.2 `agent-store`

实现四种事件 sink：

- `JsonlEventSink`：追加写入文件，并在每条事件后 flush；
- `MemoryEventSink`：供测试断言事件；
- `StdoutEventSink`：供 CLI 的 JSON 模式使用；
- `NullEventSink`：未要求事件输出时使用。

`TeeEventSink` 可以同时写 stdout 和文件。它也处理了 `events.jsonl` 这种没有父目录部分的文件名。

### 5.3 `agent-policy`

Policy 先 canonicalize 工作区根目录，再验证工具请求。默认命令 allowlist 是：

```text
python, python3, pytest, git, rg, ls, cat
```

文件路径拒绝绝对路径、`..`，并拒绝 canonical path 落到工作区外部的符号链接。命令只接受裸可执行文件名，所以 `/tmp/python3` 和 `sh` 都会被拒绝。

这里的 policy 仍然不是 OS sandbox。它能阻止工具 API 的明显越界，但一个被允许启动的子进程仍可能自行访问外部文件或网络。这个差异已经明确写进协议，避免把逻辑校验误称为系统隔离。

### 5.4 `agent-exec`

命令执行基于 Tokio process：不经过 shell，设置工作目录，捕获并合并 stdout/stderr，支持 timeout 和输出截断，并返回：

```text
success, output, exit_code, elapsed_ms, timed_out, truncated
```

进程启用了 `kill_on_drop(true)`。测试覆盖了超时和截断。不过当前只保证直接子进程的生命周期，完整进程树清理仍属于下一阶段。

### 5.5 `agent-tools`

`ToolRegistry` 同时持有 policy 和 executor，对模型暴露 4 个 OpenAI function schema，并把调用路由到具体实现。

它有一个重要语义：工具失败返回结构化 `ToolResult`，而不是把整个 turn 变成 Rust error。例如单元测试退出码为 1 时，kernel 仍然会把失败输出追加为 `function_call_output`，让模型决定是读文件、打补丁还是换一种命令。

搜索递归时跳过 `.git`，也不会跟随指向工作区外部的符号链接。补丁引擎要求上下文精确匹配，避免模糊替换改错位置。

### 5.6 `provider-openai`

这个 adapter 向 `/responses` 发起带 bearer token 的 POST，请求包含 model、instructions、完整 input history、tools 和 `stream=true`。

SSE 解析器会缓存不完整 chunk、兼容 CRLF，并按 frame 解析：

- `response.output_item.done`：提取 function call 或 message；
- `response.completed`：提取 response ID。

它最多尝试 3 次，退避 200 ms、400 ms；传输错误、HTTP 429、5xx 以及流解析/传输失败可重试，其余 4xx 直接返回错误。单元测试覆盖了 SSE 到中立 `ModelResponse` 的解析。

### 5.7 `agent-kernel`

Kernel 是核心控制循环。简化后的伪代码如下：

```text
创建 thread_id、turn_id
发出 thread.started、turn.started
history = [用户消息]

循环，最多 max_steps 次：
    response = model.complete(history, tool_specs)

    如果 response 有 tool_calls：
        对每个 call：
            history += function_call
            发出 item.started
            result = tools.execute(call)
            发出 item.completed
            history += function_call_output(result)
        继续下一轮采样

    否则：
        要求 response.text 存在
        发出 agent_message item.completed
        发出 turn.completed
        返回最终文本

未处理错误：
    发出 turn.failed
    返回错误
```

默认 `max_steps` 是 16。超过步数必须明确失败，不能静默结束。一个响应可以包含多个 tool call；kernel 会逐个产生严格匹配的 call/result 对。

### 5.8 `agent-cli`

最终二进制叫 `our-agent`。目前支持：

```text
PROMPT
--workspace PATH
--model MODEL
--base-url URL
--events FILE
--json
--allow-command NAME   # 可重复
--max-steps N
```

真实 Provider 模式从 `OPENAI_API_KEY` 读取凭据。人类输出模式打印最终消息和 `git diff --no-ext-diff`；JSON 模式只向 stdout 输出事件 JSONL，便于上层 UI 或自动化消费。指定 `--events` 时还可以把同一事件追加保存到文件。

## 6. Rust 端到端实验如何运行

Rust contract 没有直接把 mock object 注入 kernel，而是启动一个本地 HTTP/SSE Responses 兼容服务器，让编译后的真实 CLI 走完整网络 adapter。

流程是：

```text
复制初始 fixture 到临时 Git 仓库
  -> 先跑 unittest，确认 2 项失败
  -> 启动本地 Responses SSE server
  -> 运行 target/debug/our-agent
  -> 记录每次 HTTP 请求和事件 JSONL
  -> 再跑 unittest，确认 2 项通过
  -> 检查 Git diff、最终消息和所有协议断言
```

JSON 模式验证了：

- Responses 请求数为 4；
- history 长度为 1、3、5、7；
- 每次请求都携带 4 个工具 schema；
- 事件文件正好有 10 行；
- stdout 的 JSON 事件和事件文件逐条相等。

人类输出模式另外验证了最终消息、`Diff:` 标题，以及期望的删除行和新增行。

这比只测单个函数更接近真实使用路径，因为它同时经过 CLI 参数解析、HTTP 请求、SSE 流解析、kernel 循环、工具执行、文件修改、事件 sink 和最终渲染。

## 7. 当前验证结果

截至本报告生成时，以下检查全部通过：

| 检查 | 结果 |
|---|---|
| Python tests | 3 passed |
| Rust tests | 6 passed |
| `cargo fmt --all -- --check` | passed |
| `cargo clippy --workspace --all-targets --offline -- -D warnings` | passed |
| `cargo test --workspace --offline` | passed |
| Rust HTTP/SSE E2E，JSON 模式 | passed |
| Rust HTTP/SSE E2E，人类输出与 diff | passed |
| contract JSON 解析 | passed |
| shell 脚本语法检查 | passed |

Rust debug 二进制约 24 MB；完成构建、测试和 Clippy 后，本地 target 目录约 684 MB。与官方实现相比，这一规模已经足够小，可以作为我们继续掌控的产品内核。

## 8. 如何本地复现

下面的命令都在 `agent-lab/` 下运行。项目把实验工具链放在 `.tools/` 内，因此先设置对应环境变量：

```bash
export CARGO_HOME="$PWD/.tools/cargo"
export RUSTUP_HOME="$PWD/.tools/rustup"
export RUSTUP_TOOLCHAIN=stable
export PATH="$CARGO_HOME/bin:$PATH"
```

运行静态检查和 Rust 测试：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo test --workspace --offline
```

运行完整确定性 HTTP/SSE 纵向闭环：

```bash
cargo build --workspace --offline
./experiments/rust-phase1/run-contract.sh
```

脚本会在 `experiments/rust-phase1/artifacts/` 中保存事件、请求日志、测试前后输出和 fixture diff。它使用本地 contract key，不需要真实 API key，也不会请求云端模型。

要连接真实 Responses 兼容 Provider，可以使用：

```bash
export OPENAI_API_KEY="你的密钥"
./target/debug/our-agent \
  --workspace /path/to/repository \
  --model YOUR_MODEL \
  --events events.jsonl \
  "检查失败测试，做最小修复并复测"
```

真实调用前应选择一个可丢弃或已提交的测试仓库。当前版本还没有审批状态和 OS 级沙箱。

## 9. 这次实现中最重要的学习

### 9.1 Agent 的核心不是“会调用工具”，而是可验证的状态转换

最小 loop 看起来只是“模型返回工具，程序执行，再问模型”。真正决定系统是否可靠的是：谁拥有历史、call/result 怎样配对、失败如何进入下一轮、何时结束，以及外部观察者能否重建发生过的事情。把这些写成协议和 invariant 后，agent 才从 demo 变成可演进的 runtime。

### 9.2 History 和 event log 应分开

Canonical history 服务模型推理，必须保留 Provider 所需的消息和工具结果顺序。Public events 服务 UI、日志和自动化，应该稳定、简洁并允许增加字段。两者可以互相关联，但不能假设数据结构完全相同。

### 9.3 工具失败通常是数据

测试失败、搜索无结果、补丁上下文不匹配，很多时候都应该回到模型，让它修正计划。只有基础设施错误或明确终止条件才应该让 turn 失败。这个区分直接影响 agent 能否自主恢复。

### 9.4 Policy 和 sandbox 是两层能力

路径检查和命令 allowlist 是必要的，但它们不等于系统调用隔离。下一阶段必须让 policy 负责“是否允许”，让 OS sandbox 负责“即使程序有 bug，也无法越权”。

### 9.5 确定性 contract 是调研到实现之间的桥

官方源码告诉我们真实系统怎样运行；contract 把其中最重要的行为变成小而稳定的验收标准；Python 和 Rust 再分别证明协议与实现一致。这样后续增加 Anthropic adapter、审批或持久化时，可以在不依赖模型随机性的情况下发现回归。

### 9.6 不 fork 官方仓库是经过实验后的工程选择

这不是为了追求代码少，而是为了让我们只承担当前产品真正需要的复杂度。官方实现仍然可以持续充当 protocol oracle：遇到事件语义、Responses 细节或工具行为问题时，用固定实验回到上游验证，再把确认过的边界移入自己的协议。

## 10. 当前能力边界

Phase 1 已经完成：真实 CLI 能调用 Responses 兼容 Provider、运行工具、维护完整 history、修改工作区、复测、写事件并显示 diff。

以下能力尚未实现：

- allow / ask / deny 审批流和 `AwaitingApproval` 状态；
- macOS Seatbelt 等 OS 级 sandbox；
- cancellation token 和完整进程树清理；
- 显式网络 capability；
- canonical rollout 持久化、崩溃恢复和 resume；
- token budgeting 与上下文压缩；
- 新建、删除、重命名文件的完整 patch 语义；
- TUI、MCP、skills、hooks 和 subagents；
- Anthropic Messages adapter 与 Claude profile。

此外，当前事件日志记录的是公开生命周期，还没有把可恢复所需的完整 canonical history 持久化。它适合观察和验收，但还不是 crash-safe 的事实数据库。

## 11. 建议的后续推进顺序

### Phase 2：先补安全执行边界

1. 把 policy 从 allow/deny 扩展为 allow/ask/deny。
2. 增加 `AwaitingApproval`、approval request/result 及恢复执行路径。
3. 引入 macOS OS sandbox，并明确文件和网络 capability。
4. 加入 cancellation token、超时后的进程树清理。
5. 为拒绝、批准、取消和越界行为增加 deterministic contracts。

完成这一阶段后，内核才适合在更多真实仓库中由人监督运行。

### Phase 3：补持久化和长任务能力

1. 持久化完整 canonical rollout，而不只保存公开事件。
2. 为 thread/turn 建立可恢复快照和 crash recovery。
3. 加入 token 预算、history compaction 和摘要边界。
4. 保证恢复后 tool call/result 仍保持一一对应。

### Phase 4：扩展成双 Provider 产品

1. 在不修改 kernel 的前提下增加 Anthropic Messages adapter。
2. 建立 Codex profile 和 Claude profile 的行为配置。
3. 逐步加入 `CLAUDE.md`、hooks、skills、MCP 和 subagent 能力。
4. 为不同 Provider 复用同一批 kernel contract，再增加各自 adapter contract。

这个顺序的依据是：先让执行安全，再让任务可恢复，最后扩大产品表面。这样新增功能不会绕过尚未建立的基础边界。

## 12. 推荐阅读顺序

如果要从代码开始学习，建议按下面顺序阅读：

1. `PROTOCOL.md`：先理解系统承诺的行为。
2. `contracts/fix-failing-test/contract.json`：看一条 turn 如何被具体化。
3. `rust/agent-protocol/src/lib.rs`：看最小公共类型。
4. `rust/agent-kernel/src/lib.rs`：看 agent loop 和 history 更新。
5. `rust/agent-tools/src/lib.rs`：看工具结果如何结构化返回。
6. `rust/agent-policy/src/lib.rs` 与 `rust/agent-exec/src/lib.rs`：看逻辑策略和进程执行的边界。
7. `rust/provider-openai/src/lib.rs`：看 Responses SSE 怎样被归一化。
8. `rust/agent-store/src/lib.rs`：看事件如何追加记录。
9. `rust/agent-cli/src/main.rs`：看所有组件怎样组装成 CLI。
10. `experiments/rust-phase1/run-contract.sh`：看整个系统怎样被端到端验收。

读完这条路径后，可以再查看官方源码实验结果，理解为什么我们保留了这些边界，同时没有把官方仓库的全部复杂度搬进来。
