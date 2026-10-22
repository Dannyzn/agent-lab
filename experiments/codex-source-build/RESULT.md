# Codex source-build experiment result

Status: Completed on 2026-10-22 (Asia/Shanghai).

## Source and toolchain

- Upstream commit: `d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`
- Source archive SHA-256: `e3e1af2a06ec9c1bd8c510f80d520f932446e440bf58a1799c8375765f7c4316`
- Rust: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`
- Cargo: `cargo 1.99.1 (5f94df478 2026-08-27)`
- Source-built binary: `codex-cli 0.0.0`

GitHub's git transport was not reliable in this environment. Eight exact-revision Git dependencies were downloaded as source snapshots and rewritten to local path dependencies by `prepare_local_deps.py`. Only the matching Git `source` fields were removed from `Cargo.lock`; package names, versions and revision-pinned source snapshots remain recorded in `LOCAL_DEPENDENCIES`.

## Build result

`cargo build --locked --bin codex` succeeded.

- First completed compilation after dependency acquisition: 9 minutes 33 seconds.
- Incremental reproduction through `build.sh`: 2.56 seconds (`build-seconds.txt` records 2 whole seconds).
- Debug `codex` binary: approximately 1.2 GB.
- Debug `target` tree after build and schema generation: approximately 16 GB.
- Warnings came from revision-pinned third-party dependencies; no build error remained.

The snapshot contains 5,409 Rust files and 2,116,969 Rust lines across 159 workspace members. The core alone contains 595 Rust files and 256,967 lines. This is useful as a reference implementation, but it is too large to use as the starting surface for a Mini product.

## Deterministic end-to-end run

`run-local-e2e.sh` ran the source-built binary against a local deterministic Responses API. It used a disposable Git fixture and drove the real Codex sampling and tool loop through four model responses:

1. request `exec_command` to run the tests;
2. receive exit code 1 and request an `apply_patch` file change;
3. receive the patch result and request the tests again;
4. receive exit code 0 and emit the final assistant message.

Observed results:

- Codex exit code: 0.
- Trace duration: under one second (recorded as 0 or 1 whole second depending on the run).
- Raw JSONL events: 10, with zero parse errors.
- Event order: `thread.started`, `turn.started`, three `item.started` / `item.completed` tool pairs, assistant `item.completed`, `turn.completed`.
- Item types: `command_execution` failed, `file_change` completed, `command_execution` completed, `agent_message`.
- Tests before: 2 failures.
- Tests after: 2 passes.
- Patch: one line, changing subtraction to addition.
- Responses requests: 4.
- Generated app-server schema files: 318.

The request trace confirms that this Codex version sends the canonical accumulated history on every Responses request. Each later request includes the prior `function_call` followed by its matching `function_call_output`; `previous_response_id` is not used in this run. The model-facing tool set contained 10 tools, while the public JSONL stream reduced internal work to stable thread, turn and item events.

The run used `gpt-5.5` metadata because the current `gpt-6-*` presets are `code_mode_only` and require a separately built `codex-code-mode-host`. This is another example of a product feature that should not be pulled into the first Mini kernel.

## Live-provider preflight

The live preflight reached the actual Codex transport and recorded `thread.started` and `turn.started`, then timed out while connecting to `chatgpt.com`. It retried WebSocket sampling from 2/5 through 5/5, fell back to HTTPS, and timed out again. This is an environment transport limitation, not a source-build or tool-loop failure. The local Responses run isolates and verifies the protocol, event mapping and tool execution without that external dependency.

## Decision

Use upstream Codex as an executable reference and protocol oracle. Do not fork its entire workspace for the Mini implementation.

The small kernel should preserve five boundaries:

1. provider-neutral model request and response types;
2. thread / turn / item lifecycle events;
3. tool schema, router and runtime separation;
4. policy decisions separated from OS enforcement;
5. append-only JSONL as the initial fact source.

`crates/codex-mini` implements the first runnable version of this boundary: a provider interface, OpenAI Responses adapter, tool registry, workspace policy, JSONL event sink and a tested fix/retest turn loop. OS sandboxing, approval callbacks, cancellation, compaction and recovery remain explicit next increments.
