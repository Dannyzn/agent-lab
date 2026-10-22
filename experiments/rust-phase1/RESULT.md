# Rust Phase 1 result

Status: Completed on 2026-10-22 (Asia/Shanghai).

## Delivered

- `PROTOCOL.md` defines canonical history, model response, lifecycle events, tools, workspace rules and turn invariants.
- `contracts/fix-failing-test/contract.json` is a language-neutral deterministic contract.
- The Python prototype consumes the shared contract as an executable reference.
- The Rust workspace contains eight focused crates and a runnable `our-agent` binary.
- `provider-openai` implements Responses SSE, full canonical history, bearer authentication and three-attempt retry for transport, 429 and 5xx failures.
- The runtime exposes `read_file`, `search`, `apply_patch` and `exec_command`.
- Commands use argv without a shell, enforce an executable allow-list, support timeout and truncate combined output after 65,536 bytes.
- File tools reject absolute paths, parent traversal and symlinks that resolve outside the workspace.
- Events can stream to stdout and append to JSONL.
- Human CLI output includes the final message and Git diff.

## Contract evidence

The same contract passed through three layers:

1. Python kernel with an in-process scripted provider.
2. Rust kernel with an in-process scripted provider.
3. Rust `our-agent` CLI through a local HTTP/SSE Responses-compatible server.

The end-to-end run observed:

- 4 Responses requests;
- canonical history lengths 1, 3, 5 and 7;
- 4 tool schemas on every request;
- 10 public events in the specified order;
- 2 failing tests before the turn;
- a one-line patch;
- 2 passing tests after the turn;
- final message and Git diff in human output.

## Size

- Rust implementation: 8 source files, approximately 1,300 lines.
- Debug binary: approximately 24 MB.
- Local target directory after build, test and Clippy: approximately 684 MB.

For comparison, the pinned official Codex snapshot contained 159 workspace members and produced a roughly 1.2 GB debug binary with a 16 GB target directory in this environment.

## Validation

- Python tests: 3 passed.
- Rust tests: 6 passed.
- `cargo fmt --check`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- Rust HTTP/SSE end-to-end contract: passed in JSON and human output modes.

## Phase boundary

This completes the Phase 1 vertical slice: a real CLI can call a Responses-compatible provider, execute tools, maintain history, write events, modify a workspace, run tests and show a diff.

It is not yet safe for unattended use on arbitrary repositories. Phase 2 must add allow / ask / deny approvals, OS sandbox enforcement, cancellation with process-tree cleanup and explicit network capabilities. Phase 3 will add canonical rollout persistence, resume, compaction and crash recovery.
