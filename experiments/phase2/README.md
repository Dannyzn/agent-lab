# Phase 2 acceptance

目标：对真实 `our-agent` CLI 验证审批、macOS Seatbelt 和取消。模型端是本机确定性 Responses HTTP/SSE 服务，无云端 key 和模型随机性。

在仓库根目录先构建 `cargo build --workspace`，再执行：

```bash
./experiments/phase2/run-contract.sh
```

需要支持 Seatbelt 的 macOS 宿主环境及回环端口权限；外层 sandbox 可能禁止嵌套 Seatbelt，应在宿主终端执行，不能为使测试通过而关闭本项目 sandbox。

13 个 CLI 测试：

1. 批准前无副作用，批准一次执行，history 和 stdout JSONL 对齐。
2. 拒绝与错误 approval ID 不产生写入。
3. 审批超时终止 turn，不留下一次审批去竞争 stdin reader。
4. 审批等待时取消，无文件改动，approval/item 均关闭。
5. 模型等待时 SIGTERM 立即终止。
6. 命令运行时 SIGINT 清理同组子孙进程。
7. 人类模式 `y` 批准。
8. `never` 直接拒绝且不发出审批。
9. 只读模式不能被 auto 批准绕过。
10. 即使批准命令，工作区外写入仍被 Seatbelt 拒绝。
11. auto 模式仍拒绝未列入白名单的命令。
12. stdin EOF 默认拒绝。
13. 拒绝把审计事件文件放在工具可写工作区内。

Rust 测试另外验证外部文件内容读取拒绝、外部写入拒绝、Git 元数据保护、环境变量清理、网络 deny / allow、超时 / 正常父进程退出后的同组后代清理，以及有界输出。

`artifacts/`（Git 忽略）保留每次实验的 events、requests、stdout、stderr 与 summary。读取进行中的 JSONL 时，只消费换行完整的行。
