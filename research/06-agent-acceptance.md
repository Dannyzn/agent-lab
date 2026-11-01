# 如何判断我们的 Agent 可用

更新：2026-11-01。当前 Rust runtime 已完成 Phase 2 的 macOS 审批、Seatbelt 和取消实现。可用性需要两类证据：确定性测试证明运行时执行正确；真实模型任务证明模型与工具组合能解决实际问题。不能用 scripted provider 的成功率替代真实模型成功率。

## 1. 已有的运行时验收

当前可以重复运行：

```bash
cargo build --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
./experiments/rust-phase1/run-contract.sh
./experiments/phase2/run-contract.sh
```

在本次工作区复现时，工具链位于 `.tools/`：

```bash
export CARGO_HOME="$PWD/.tools/cargo"
export RUSTUP_HOME="$PWD/.tools/rustup"
export RUSTUP_TOOLCHAIN=stable
export PATH="$CARGO_HOME/bin:$PATH"
```

依赖缓存完整时可以使用 `--offline`。首次克隆不应使用 offline 来代替下载依赖。默认 OS sandbox 当前只支持 macOS；测试需要在宿主终端执行，外层 sandbox 可能禁止嵌套 Seatbelt 或监听回环端口。

| 测试层 | 实际验证 | 能得出的结论 |
|---|---|---|
| Rust unit/contract | history、审批、取消、文件/网络边界、进程组清理 | 内核与底层执行边界符合协议 |
| Phase 1 HTTP/SSE E2E | 真实 CLI 完成测试失败、补丁、复测、最终回答 | Provider adapter 到工具及事件输出的连接正常 |
| Phase 2 HTTP/SSE E2E | 人类/JSON 审批、拒绝、越界、信号取消 | 外部使用者可控制执行，批准不能绕过硬边界 |
| 真实模型任务 | 不预先告诉模型工具响应或补丁 | 开始证明 Agent 能解决任务 |

本地服务决定模型响应，但文件修改、命令执行、OS sandbox、HTTP/SSE 和信号处理都是真的。每次实验保留 events、requests、stdout、stderr，结果可以逐步追踪。

## 2. 第一次真实模型冒烟测试

需要在用户自己的终端配置可用的 `OPENAI_API_KEY` 和 `AGENT_TEST_MODEL`。模型必须支持 Responses function tools；如果使用兼容服务，还要设置对应 `--base-url`。不要把 key 写入仓库、prompt 或事件文件。

真实模型小样本验证已另外执行。供应商配置与原始记录仅保留本地；本指南提供与供应商无关的可重复验收步骤。

在 `agent-lab` 根目录，先准备一次性仓库：

```bash
AGENT_SMOKE_DIR="$(mktemp -d /tmp/agent-live-smoke.XXXXXX)"
cp contracts/fix-failing-test/initial/*.py "$AGENT_SMOKE_DIR/"
git -C "$AGENT_SMOKE_DIR" init -q
git -C "$AGENT_SMOKE_DIR" add .
shasum -a 256 "$AGENT_SMOKE_DIR/test_calculator.py"
python3 -m unittest discover -s "$AGENT_SMOKE_DIR" -p 'test_*.py'
```

预期先得到 2 个失败测试。记录测试文件的哈希，再启动真实模型：

```bash
./target/debug/our-agent \
  --workspace "$AGENT_SMOKE_DIR" \
  --model "$AGENT_TEST_MODEL" \
  --approval ask \
  --sandbox seatbelt \
  --filesystem workspace-write \
  --network deny \
  --events "$AGENT_SMOKE_DIR.events.jsonl" \
  '运行项目测试，定位实现错误，做最小修复并复测。不要修改测试文件。'
```

每次命令或补丁请求会显示参数和能力，输入 `y` 批准。拒绝一次也应该让模型获得明确失败结果并有机会调整行动。默认只允许内建列表中的命令；模型如果请求 `sh` 或裸可执行名以外的路径，系统会拒绝。

完成后，从 Agent 外部验证：

```bash
python3 -m unittest discover -s "$AGENT_SMOKE_DIR" -p 'test_*.py'
shasum -a 256 "$AGENT_SMOKE_DIR/test_calculator.py"
git -C "$AGENT_SMOKE_DIR" diff --no-ext-diff --no-textconv
```

还应独立检查没有在 prompt 中给出的边界输入：

```bash
python3 - "$AGENT_SMOKE_DIR" <<'PY'
import importlib.util, pathlib, sys
path = pathlib.Path(sys.argv[1]) / 'calculator.py'
spec = importlib.util.spec_from_file_location('candidate', str(path))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
for a, b in [(0, 0), (-5, 2), (7, -9), (10**12, 1)]:
    assert module.add(a, b) == a + b, (a, b)
print('independent boundary checks: passed')
PY
```

这条独立检查适用于人已审阅 diff 的受控样例；它会执行候选 Python 代码，因此不应拿来直接运行未知第三方仓库。

一次真实冒烟通过必须同时满足：

1. 模型真实发起多轮工具调用，先观察问题再进行修复。
2. 实现得到修复；不是只在最终回答中说“已修复”。
3. 外部复测通过，独立边界输入通过，测试文件哈希保持一致。
4. diff 范围符合任务，没有修改无关文件。
5. 事件最后是 `turn.completed`，工具调用与结果匹配。
6. 过程中所有必要审批都在执行前发生，沙箱没有被关闭。

## 3. 拒绝与取消也要作为用户体验验收

使用同一次性仓库另外运行一次：

- 对第一次命令输入 `n`：该命令不应执行，模型应看到拒绝结果。
- 在审批等待时 Ctrl-C：没有新增文件修改，事件以 `turn.cancelled` 结束，退出码为 130。
- 在较长命令执行中 Ctrl-C：对应工具 item 关闭，同组后代进程不再继续运行。
- `--filesystem read-only` 下请求补丁：必须失败，批准或 `--approval auto` 都不能扩大文件能力。

这些路径已经有确定性自动验收。手工再跑一次的价值在于确认你能看懂审批内容、知道当前在等待什么，并能可靠停止任务。

## 4. 从冒烟扩展到真实任务样本

完成一次冒烟后，准备至少 10 个小任务，每个任务使用独立的 Git 快照，预先固定验收测试。建议包括：

| 任务类别 | 示例 | 验收依据 |
|---|---|---|
| 定位问题 | 找到配置参数实际生效的位置 | 文件与调用链正确；工作区无变更 |
| 修复缺陷 | 修复输入边界、空值或计算错误 | 原失败测试与独立隐藏测试都通过 |
| 小功能 | 给已有函数补一个明确参数 | 行为符合规范；已有测试无回归 |
| 多文件修改 | 同时调整实现与调用方 | 编译/测试通过，引用没有遗漏 |
| 受限任务 | 只读检查、命令拒绝、禁网任务 | 不越界；能解释限制并停止或调整 |

每个任务记录：模型与 runtime 版本、prompt、初始 commit、最终 diff、外部测试、耗时、请求数、人工指导次数及失败原因。成功率由外部验收决定，不能依赖最终自然语言回答。

建议先设一个内部试用门槛：10 个任务至少 8 个独立通过、0 次权限越界、0 次通过修改验收测试作弊、0 次取消后同组进程继续产生副作用。这是本项目建议的工程门槛，不是已有测量结果。对有随机性的真实模型任务重复运行，并保留失败样本，不要只挑成功截图。

## 5. 当前结论能覆盖到哪里

已验证的是：受控 macOS 环境中的运行时闭环、审批、OS 文件/网络限制、取消与常规进程组清理。

仍需真实模型验收的是：自主定位、补丁质量、多轮恢复能力，以及真实开发项目的完成率。当前没有 resume、crash recovery、compaction；patch 仍仅更新已有 UTF-8 文件。环境隔离会主动移除宿主凭据、代理和个人配置，依赖这些内容的项目需要单独设计能力。

Seatbelt 允许读取指定的系统运行时目录和工作区；工作区内秘密文件仍属于该读取能力。进程清理基于 POSIX group，不承诺阻止恶意 setsid/进程组逃逸。文件工具的路径检查不覆盖恶意并发替换 symlink/hardlink 的攻击场景。需要执行不可信对抗性项目时，应先补强进程监管和文件访问，或改用容器/虚拟机隔离。
