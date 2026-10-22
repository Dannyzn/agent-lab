# Official Codex source build and end-to-end trace

This experiment builds an exact snapshot of `openai/codex`, runs a bounded coding task in a disposable fixture, and records Codex JSONL events.

## Pinned source

- Repository: `https://github.com/openai/codex`
- Commit: `d138398cef7d49e46fe33f1d0ddbd2ca0ed16aa4`
- Snapshot date: 2026-10-21
- Local source: `../../upstream/codex`

The source snapshot is ignored by this repository. `SOURCE_COMMIT` records the exact revision separately from the downloaded tree.

## Commands

From the `agent-lab` root:

```bash
experiments/codex-source-build/build.sh
experiments/codex-source-build/run-local-e2e.sh
experiments/codex-source-build/run-trace.sh
```

`build.sh` uses the workspace-local Rust toolchain under `.tools/` and builds only the `codex` binary. `run-local-e2e.sh` drives that binary through a deterministic local Responses API and exercises the complete test/fix/retest tool loop without depending on external model connectivity. `run-trace.sh` performs the same bounded task against the configured live provider. Both trace scripts use a fresh fixture copy and record:

- raw JSONL events;
- stderr/runtime logs;
- before/after tests and Git diff;
- a compact event timeline generated from the raw trace;
- generated app-server JSON Schema.

Generated data is written to `artifacts/` and is intentionally ignored by Git.

## Safety boundary

The deterministic trace runs without Codex's nested sandbox because this environment does not permit a second macOS Seatbelt profile. Its working directory is a disposable fixture copy inside this already sandboxed workspace, and the mock server can only request the three fixed commands encoded in `mock_responses_server.py`. The live task uses `workspace-write`. Neither path publishes code.

## Result

See `RESULT.md` after the experiment finishes. It records observed build time, binary version, trace counts, lifecycle, and implementation implications.
