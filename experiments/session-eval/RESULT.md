# Durable-session live acceptance results

Date: 2026-11-02. Runtime: v0.3.0 baseline and v0.3.1 compaction fix. The configured live model was used; connection details, model identity, credentials, raw traces and generated candidate implementations remain local. Provider-neutral selected metrics are in [results.json](results.json).

## Observations, including failures

| Scenario / implementation | Local context budget | Model steps | Compactions | Seconds | Result |
|---|---:|---:|---:|---:|---|
| v0.3.0 invoice repair, crash, resume | 32768 | 7 | 0 | 47.69 | strict pass |
| v0.3.0 same-thread follow-up | 32768 | 7 additional | 0 | 39.92 | strict pass |
| v0.3.0 long documents, crash, resume | 16000 | 32 | 16 | 165.34 | failed at step limit |
| Development iteration: file evidence only | 16000 | 32 | 20 | 174.70 | failed at step limit |
| Development control: no interruption | 65536 | 23 | 0 | 133.63 | strict pass |
| v0.3.1 file evidence + continuation, crash, resume | 16000 | 19 | 10 | 111.43 | strict pass |

Every turn has a 32-step limit and a 4096 output-token limit per response. Context values are conservative local estimates, not measured model-token usage. Follow-up raw metrics are cumulative for the thread; this table reports its seven additional steps. These are individual observations across development iterations, not a success-rate benchmark or statistical proof.

The control changed both context pressure and interruption exposure; it demonstrates that the model could complete this fixture under those conditions. It does not isolate those factors by itself. The final stress run retained the original prompt, 16000 budget, 32-step limit, interruption rule, file protections and external oracle.

## What passed

- Invoice: all three required implementation files changed, visible tests and the independent pricing/service/CLI oracle passed, protected files remained unchanged. The process was killed after a durable edit while waiting for an ungranted approval. Resume kept the same turn, used a new approval ID, and did not repeat the earlier patch.
- Follow-up: the same thread added one-shot iterable support and negative/non-integer price rejection. Two required implementation files changed, the prior CLI stayed unchanged, and both the original oracle and the extended generator/validation checks passed. Two turns completed on one thread.
- Shipping: all six long policy documents were successfully read in full. After an actual compaction and SIGKILL at an approval, the task resumed and changed both implementation files. The final stress run passed all three visible tests and the external oracle: 192 zone/weight/threshold combinations, 12 invalid API cases, 12 valid CLI cases and 3 invalid CLI cases. All protected hashes matched; there were no denied approvals. All ten post-compaction estimates were at or below 16000; the maximum was 15718.

Tool failures include expected initial failing tests; they are not all runtime failures. The final shipping trace also contains some commands that discovered zero tests, followed by explicit discovery of three tests and successful verification. Acceptance relies on the host's independent test command and oracle, not an exit-zero claim from the model.

## What the failures taught us

The original summary retained only the last six tool envelopes, clipped to JSON head/tail. Important cross-file rules could disappear. The first failure repeatedly read documents and left only a rate table; the business function and CLI remained unfinished. A first fix retained file facts but still restarted inspection and made no edits before exhausting the budget.

The final v0.3.1 change:

1. Decode tool envelopes before extracting content.
2. Keep bounded evidence from up to twelve distinct successful file reads, merging repeated reads by path. In text/config documents, select literal key/value lines; mark snapshots that precede later recorded edits.
3. Retain the last command, historical failure and edited paths. Share the digest budget across files.
4. Append a fixed runtime continuation notice, containing no workspace text, that reminds the model the task is ongoing and completed actions already happened. Rebuild from canonical history, so this notice does not accumulate.
5. Preserve every user message and full canonical history; keep the call/result pairing, pre-request budget check and durable compaction record unchanged.

The bundle passed the next controlled stress observation. This supports keeping the fix and expanding evaluation; it does not prove the notice alone caused the improvement or that all long tasks now work. Literal extraction is lossy and can miss natural-language constraints, tables or nested configuration.

## Deterministic regression

- Rust workspace: 25 passed, including cross-file middle-of-document field retention, stale-snapshot labeling, decoded error evidence, repeatable view reconstruction and unchanged canonical history.
- Existing Phase 3 real CLI suite: 11 passed.
- New session evaluation harness: 5 tests passed, including real CLI crash/resume and compaction/control runs, a duplicate-result rejection, full-read validation, and a deliberately wrong implementation that passes visible tests but fails the external oracle.
- Existing approval-scope harness: 3 passed.
- `cargo fmt --check`, clippy with warnings denied, and diff checks passed.

The final live binary and runtime source diff were checked against the captured SHA-256 values before committing. The baseline source commit and hashes are recorded in the sanitized metrics; private artifacts retain the exact source patch for the uncommitted build used in that observation.

## Limits and next step

SIGKILL can leave subprocesses running. Injection here deliberately occurs at a waiting approval with no executing command; it does not add a general process-supervision guarantee. Previous cancellation/unknown-result boundaries still apply.

There is one final passing observation per scenario, alongside two retained long-task failures. Test more repositories, randomize task variants and repeat runs before estimating reliability. The next feature phase remains the Anthropic-compatible adapter, with the same external acceptance requirements.
