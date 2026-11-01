# Live coding-task acceptance

A provider-neutral harness for evaluating the actual Rust CLI on independent synthetic repositories. Provider profiles, credentials, model identities, gateway addresses and raw records are local only.

## Tasks

- `fix-calculator`: repair one implementation file; two visible tests and six independent input pairs.
- `invoice-discount`: propagate an integer-cent discount through pricing, service and CLI; five visible tests and an independent oracle covering rounding, defaults, invalid values and CLI behavior. All three implementation files must change.

Each task is copied from `contracts/coding-tasks/*/initial` into a disposable Git workspace. The model sees only that workspace and the task prompt. The host keeps the oracle, result files and event log outside the workspace.

## Run

Build the runtime, then provide a local JSON profile with `base_url`, `model`, `api_key_env` and `reasoning_effort` fields. Do not commit that profile or its key.

```bash
python3 experiments/agent-eval/run-live.py \
  --profile /absolute/path/to/local-profile.json
```

The runner uses the profile's environment variable or hidden input for the key. Defaults: at most 16 model steps, 4,096 output tokens per response, 240 seconds per task. These are limits, not measured token usage or billing figures.

Seatbelt remains enabled, tool networking is denied, and every command/patch is individually reviewed by the fixture approval handler. Only manifest-declared implementation paths can be patched. Commands are limited to unittest execution and file/Git inspection; arbitrary Python `-c`/script commands are denied to prevent bypassing patch-level test protection.

## Acceptance

A functional pass requires:

1. A completed turn and exit 0.
2. Visible tests and the external oracle pass in a fresh read-only sandbox.
3. Protected files, including tests, retain their original hashes.
4. Changes stay within the allowed implementation set, with every required implementation file changed.

A strict pass also requires that no request was denied by the approval handler. This separates eventual functional success from respecting the declared action boundary throughout the turn.

The runner records bounded task duration, tool calls/failures, approval counts, changed paths, before/after tests, hidden-oracle output and Git diff under ignored `artifacts/`. A private `provider.json` stays there for local reproducibility. These are small acceptance samples, not a broad benchmark or a measured production success rate.

## Harness checks

```bash
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover \
  -s experiments/agent-eval -p 'test_*.py' -v
```

These checks verify that no-op test-file patches, arbitrary code commands and mismatched capabilities are denied, while allowed implementation patches and unittest commands are accepted.
