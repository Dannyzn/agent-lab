# Independent coding-task fixtures

Each task directory contains:

- `task.json`: prompt, editable paths, required changed paths and visible-test command.
- `initial/`: all files exposed to the evaluated Agent.
- `oracle.py`: host-side independent checks, never copied into the model workspace.

Initial visible tests intentionally fail. The public test files are protected by the evaluation harness, and their hashes are compared after the task. The oracle verifies additional inputs and behavior without relying on the Agent's final message.

`fix-calculator` exercises the basic failure/inspection/patch/retest loop. `invoice-discount` requires a consistent change across pricing, service and CLI, including integer arithmetic and error propagation.

Provider-specific profiles and live traces are excluded from version control. See `experiments/agent-eval/README.md` for the generic runner.
