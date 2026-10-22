# Rust Phase 1 end-to-end contract

这个实验通过 HTTP/SSE 调用 `provider-openai`，经过 Rust kernel 和真实工具执行，验证 `our-agent` CLI 的完整纵向链路。

```bash
cargo build -p agent-cli --offline
experiments/rust-phase1/run-contract.sh
```

成功条件：

- 四次 Responses 请求的 canonical history 长度为 1、3、5、7；
- 四个工具 schema 每轮都存在；
- JSONL 事件符合 `PROTOCOL.md`；
- 初始两个测试失败，修改后两个测试通过；
- Git diff 只有预期的一行实现修复。
