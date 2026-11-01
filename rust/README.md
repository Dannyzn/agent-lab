# Rust runtime v0.2

`PROTOCOL.md` 的实现，当前支持 macOS。9 个 crate：protocol、store、policy、sandbox、exec、tools、provider-openai、kernel、cli。

## 构建与检查

使用已安装的 Rust toolchain，在仓库根目录执行（首次克隆需要下载依赖）：

```bash
cargo build --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
./experiments/rust-phase1/run-contract.sh
./experiments/phase2/run-contract.sh
```

本次实验使用工作区内的工具链与缓存，复现时可先设置：

```bash
export CARGO_HOME="$PWD/.tools/cargo"
export RUSTUP_HOME="$PWD/.tools/rustup"
export RUSTUP_TOOLCHAIN=stable
export PATH="$CARGO_HOME/bin:$PATH"
```

缓存完整时，上述 Cargo 命令可加 `--offline`。OS 沙箱与回环端口测试需要在支持 Seatbelt 的宿主终端运行；已有外层沙箱可能拒绝嵌套 `sandbox_apply`，这不等同于测试通过。

## 运行

先在终端提供 `OPENAI_API_KEY`，并指定账户可用的 `AGENT_TEST_MODEL`：

```bash
./target/debug/our-agent \
  --workspace /absolute/path/to/disposable-project \
  --model "$AGENT_TEST_MODEL" \
  --events /tmp/our-agent-events.jsonl \
  '检查项目，修复失败测试并复测'
```

默认每次命令 / 补丁前询问，stdin 输入 `y` 批准一次，其余输入拒绝。CLI 要求事件文件放在工具工作区外，防止工具篡改审计记录。

| 参数 | 默认 | 含义 |
|---|---|---|
| `--approval` | `ask` | `ask` 逐次询问；`never` 拒绝需审批调用；`auto` 在已有能力内自动执行 |
| `--sandbox` | `seatbelt` | macOS OS 隔离；`none` 显式关闭且发出警告 |
| `--filesystem` | `workspace-write` | `read-only` 禁止工作区写入，或允许工作区写入 |
| `--network` | `deny` | 工具子进程禁网或显式 `allow`；不影响宿主模型请求 |
| `--approval-timeout-ms` | `120000` | 审批超时终止 turn，范围 1–600000 |
| `--allow-command` | 内建列表 | 重复指定可替换全部命令列表；可执行文件必须在固定 PATH 中 |

Seatbelt 支持文件系统隔离和进程继承；外部文件内容不可读写，系统运行时目录可读，独立 scratch 目录可写，Git 元数据不可写。子进程环境不继承 API key、代理变量和个人 Git 配置。需要额外依赖、证书、代理或包缓存的项目应先明确能力设计。

## JSON 审批与取消

`--json` 只向 stdout 输出 JSONL。收到 `approval.requested` 后，宿主向 stdin 回传：

```json
{"approval_id":"与请求完全一致的ID","decision":"allow"}
```

也可回传 `deny`。无效或不匹配的回复一律拒绝。Ctrl-C（SIGINT）和 SIGTERM 会取消 turn，关闭正在执行的 item，输出 `turn.cancelled`，以 130 退出。

进程组清理覆盖常规子孙进程；不保证恶意 `setsid`/进程组逃逸。同步文件工具在工具边界观测取消，已落盘修改不自动回滚。当前还没有 resume、crash recovery 或 compaction。

## 兼容服务与本地配置

`--base-url` 与 `--model` 可连接 Responses 兼容服务。使用 `--max-output-tokens` 限制单次输出，按服务能力设置 `--reasoning-effort`。凭据通过宿主环境变量读取，不写入仓库。

本地供应商配置、模型标识、网关地址、专用启动脚本和原始实测材料不纳入版本控制。默认审批、沙箱和工具禁网策略不因切换模型而改变。
