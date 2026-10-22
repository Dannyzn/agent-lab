# Rust runtime

这是 `PROTOCOL.md` 的生产向实现。workspace 包含八个小 crate：

- `agent-protocol`：provider-neutral 类型和事件接口
- `agent-store`：JSONL、stdout 和内存 event sink
- `agent-policy`：workspace 与命令约束
- `agent-exec`：无 shell 命令、超时和输出截断
- `agent-tools`：四个基础工具和 schema
- `provider-openai`：Responses SSE adapter
- `agent-kernel`：canonical history 和 turn loop
- `agent-cli`：`our-agent` 命令

## Build and test

```bash
export CARGO_HOME="$PWD/.tools/cargo"
export RUSTUP_HOME="$PWD/.tools/rustup"
export RUSTUP_TOOLCHAIN=stable
export PATH="$CARGO_HOME/bin:$PATH"

cargo test --workspace --offline
```

## Run

```bash
export OPENAI_API_KEY=...
cargo run -p agent-cli -- \
  --workspace /absolute/path/to/project \
  --events /tmp/our-agent-events.jsonl \
  "运行失败测试，修复问题并复测"
```

增加 `--json` 会把生命周期事件写到 stdout。命令工具只接受 argv，不经过 shell；默认可执行文件白名单是 `python`、`python3`、`pytest`、`git`、`rg`、`ls` 和 `cat`。

当前版本落实 Phase 1 纵向闭环，尚未提供 approval、OS sandbox、取消、恢复和 compaction。
