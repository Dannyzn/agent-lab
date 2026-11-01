# Durable-session coding acceptance

This runner evaluates the real Rust CLI against an explicitly configured live model. Credentials are read from the profile's environment variable or hidden terminal input. Profiles, model identities, endpoints, raw traces and candidate code stay under ignored local paths.

## Scenarios

1. **Repair and resume**: implement the three-file invoice task. After an edit has completed, stop the CLI with SIGKILL while it awaits the next ungranted approval. Resume the same turn; require a new approval ID and independent functional acceptance.
2. **Same-thread follow-up**: in that completed session, add one-shot iterable support and price validation while preserving the discount API. Require a second completed turn, protected-file hashes, two changed implementation files, and the original plus extended external oracles.
3. **Compaction and resume**: implement shipping quotes from six synthetic long rate-card documents. Set a 16,000 local context budget, require full successful reads of every document, interrupt at an approval after a compaction event, then resume. The external oracle checks all zones, weight boundaries, free thresholds, invalid arguments and CLI behavior.

The shipping fixture deliberately stresses retaining information from several long documents. It is a small synthetic task, not evidence about performance on a large real repository.

## Run

Build `our-agent`, then provide a local JSON profile with `base_url`, `model`, `api_key_env`, and optional `reasoning_effort`. Do not commit profiles or keys.

```bash
python3 experiments/session-eval/run-live.py \
  --profile /absolute/path/to/private-profile.json

# Repair/resume and follow-up only:
python3 experiments/session-eval/run-live.py \
  --profile /absolute/path/to/private-profile.json --case repair-resume

# Long-context only:
python3 experiments/session-eval/run-live.py \
  --profile /absolute/path/to/private-profile.json --case long-context
```

Each process has a 360-second default deadline; the whole case may use two processes for interruption/resume. A turn has at most 32 model steps, including the interrupted process, and each response has a 4,096-token output limit. These are configured bounds, not measured token usage or billing figures.

Seatbelt remains enabled with tool networking denied. The fixture-scoped approval handler only permits implementation patches and constrained test/inspection commands. The host keeps its oracle and result files outside the model workspace, compares protected hashes and checks the final edit scope. Independent tests run read-only with a clean environment.

A functional pass requires exit 0, visible tests, the external oracle, protected-file preservation and required edits. A strict pass also requires no denied approvals, valid call/result pairing, unique event IDs, and the scenario-specific interruption/renewal/compaction conditions. No injection reached means that scenario did not pass, even if the code happens to work.

For a diagnostic comparison without injected interruption, use `--case long-context-control`. It uses a 65,536 local context budget and reports separately from the 16,000-budget stress case. Verify the recorded compaction count is zero before treating it as an uncompacted control; a larger budget is not a guarantee of zero compactions. This comparison changes both compaction exposure and interruption, so it does not isolate those two factors by itself.

Raw output lives under `experiments/session-eval/artifacts/run-*`. A summary records the runtime commit, whether tracked source was modified, binary SHA-256, process results, and bounded metrics. Model steps and tool counts in a follow-up result are cumulative for the thread; elapsed process time belongs to that turn. The runner does not record authorization headers or save the input key.

## Offline checks

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s experiments/session-eval -p 'test_*.py' -v
```

These use loopback scripted Responses and real CLI processes. They prove that the crash injection and compaction scenario are reached, approvals are renewed, duplicate results are rejected, and merely declaring a read is insufficient. A deliberate wrong implementation passes all visible shipping tests but fails the external oracle. Tests need macOS Seatbelt and permission to bind loopback ports; they do not use a cloud credential.

Results and retained failure samples: [RESULT.md](RESULT.md).
