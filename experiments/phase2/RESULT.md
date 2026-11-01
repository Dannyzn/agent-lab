# Rust Phase 2 result

Status: macOS implementation and deterministic acceptance completed on 2026-11-01 (Asia/Shanghai).

## Delivered

- Protocol v0.2: per-call approvals, approval transport, cancellation terminal event and item closure.
- Default ask mode before patches/commands; allow/ask/deny policy with hard-deny precedence.
- Human stdin approvals plus correlated JSON stdin/stdout approvals; malformed replies and EOF deny, timeout terminates the turn.
- New `agent-sandbox` crate using macOS Seatbelt; no automatic unsandboxed fallback.
- Host-owned read-only/workspace-write and deny/allow network capabilities; tool approval cannot widen them.
- Minimal child environment, fixed executable search paths, private HOME/TMPDIR, no inherited provider API key.
- SIGINT/SIGTERM cancellation during model sampling, approval and command execution; CLI exit 130.
- POSIX process-group cleanup on cancellation, timeout, normal parent exit and dropped execution futures.
- Incremental bounded stdout/stderr capture; bounded pipe drain after group cleanup.
- Protected Git metadata, audit log outside tool workspace, and a sandboxed, read-only final Git diff command.
- Reproducible CLI acceptance harness and real-model usability test guide.

## Evidence

See `artifacts/summary.json` for the CLI suite and `artifacts/` per-run events, requests and outputs. Artifacts are local and Git-ignored.

- Rust workspace tests: 16 passed (6 executor, 4 kernel, 1 policy, 1 sandbox, 1 store, 3 provider).
- Phase 2 real CLI HTTP/SSE tests: 13 passed.
- Phase 1 HTTP/SSE fix/retest regression: passed with default Seatbelt, 4 requests, 10 events, JSON and human diff modes.
- Python reference tests: 3 passed.
- Formatting, Clippy with warnings denied, build, shell syntax and Git whitespace checks: passed.

The model responses are deterministic, but CLI, network adapter, tool execution, filesystem changes, Seatbelt enforcement and signals are real. These deterministic checks are independent of live-provider validation. Provider-specific live configurations and raw records remain local and are excluded from version control.

## Scope and remaining work

This phase supports local supervised macOS use. Linux enforcement is not implemented and default execution fails closed there. `--sandbox none` is a warned, explicit development opt-out; child filesystem/network flags are then unenforced by the OS.

This is not a hostile multi-tenant isolation boundary. Process-group escape via setsid is outside cleanup guarantees; system runtime read paths and workspace data are explicitly readable. Hostile concurrent symlink/hardlink mutation is not covered by file tools' path validation. Synchronous file tools observe cancellation at operation boundaries; committed edits are not rolled back.

Phase 3 remains canonical rollout persistence, resume, crash recovery and compaction. Real-model benchmark acceptance is separate from this deterministic runtime acceptance.
