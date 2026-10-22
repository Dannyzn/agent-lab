# Claude Code 的开源边界与兼容路线

调研日期：2026-10-21。

## 结论

Claude Code 的公开 GitHub 仓库不等于开源实现。仓库的许可证只有一行：Anthropic 保留全部权利，使用受商业条款约束。仓库当前主要承载发行说明、安装入口、plugins 和 issue 跟踪，不能像 Apache-2.0 的 Codex 那样直接 fork 后作为我们产品内核。

因此我们对 Claude Code 的目标应定义为“行为兼容 profile”：使用 Anthropic 的公开 API、文档、SDK 类型和扩展协议，独立实现相同类别的能力。

## 可用与不可用的材料

| 材料 | 许可/状态 | 对我们的价值 |
|---|---|---|
| `anthropics/claude-code` | All rights reserved | 行为、命令、发行说明和插件样例；不作为可复制内核 |
| Claude Code 官方文档 | 公开文档 | CLI 行为、权限模式、hooks、MCP、skills、subagents 的兼容规格 |
| Claude Agent SDK TypeScript | 商业条款 | 可作官方集成层，不作为自由可修改的核心 |
| Claude Agent SDK Python | MIT | 客户端类型与进程协议可学习，但它仍依赖 Claude Code CLI，不包含完整 agent harness |
| Anthropic API SDK | MIT | 实现原生 Messages API provider |
| Anthropic sandbox-runtime | Apache-2.0 | 后续可评估其进程沙箱设计 |

## 需要兼容的核心行为

Claude profile 不需要在第一版复刻全部 UI。影响 agent 结果和生态兼容性的能力优先级如下：

1. `CLAUDE.md` 指令发现与作用域合并。
2. Anthropic Messages API 的流式 tool-use 循环。
3. `Read`、`Write/Edit`、`Bash`、`Glob/Grep` 的工具语义。
4. `default`、`acceptEdits`、`plan`、`dontAsk` 等权限模式。
5. session 持久化、resume、非交互 JSON/stream-json 输出。
6. hooks：`PreToolUse`、`PostToolUse`、`Stop`、`PreCompact`。
7. skills、slash commands、MCP 和 subagents。

Chrome 集成、remote control、云会话和 IDE 特有功能可放到后期。

## 可参考的真正开源 agent

这些项目不能代替对 Codex 源码的阅读，但适合比较模型无关 provider、TUI 和编辑策略：

| 项目 | 许可证 | 值得比较的部分 |
|---|---|---|
| [OpenCode](https://github.com/anomalyco/opencode) | MIT | 多 provider、终端 UI、Claude Code 风格产品体验 |
| [Cline](https://github.com/cline/cline) | Apache-2.0 | CLI/IDE/SDK 共用 agent engine、审批与 MCP |
| [goose](https://github.com/block/goose) | Apache-2.0 | Rust agent、扩展机制、模型与工具解耦 |
| [Aider](https://github.com/Aider-AI/aider) | Apache-2.0 | repo map、编辑格式、git 驱动的回滚体验 |

在开始复制任何具体实现前，需要固定所采用的仓库 commit 并再次核验该文件或目录的许可证。

## 一手来源

- [Claude Code 仓库](https://github.com/anthropics/claude-code)
- [Claude Code LICENSE](https://github.com/anthropics/claude-code/blob/main/LICENSE.md)
- [Claude Code CLI reference](https://code.claude.com/docs/en/cli-usage)
- [Claude Agent SDK Python](https://github.com/anthropics/claude-agent-sdk-python)
- [Claude Agent SDK TypeScript](https://github.com/anthropics/claude-agent-sdk-typescript)
- [Anthropic TypeScript API SDK](https://github.com/anthropics/anthropic-sdk-typescript)
- [Anthropic sandbox-runtime](https://github.com/anthropics/sandbox-runtime)
