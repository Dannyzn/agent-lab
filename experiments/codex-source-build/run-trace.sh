#!/usr/bin/env bash
set -euo pipefail

experiment_dir="$(cd "$(dirname "$0")" && pwd)"
lab_root="$(cd "$experiment_dir/../.." && pwd)"
codex_bin="$lab_root/upstream/codex/codex-rs/target/debug/codex"
artifact_dir="$experiment_dir/artifacts"

export PYTHONDONTWRITEBYTECODE=1

if [[ ! -x "$codex_bin" ]]; then
  echo "Source-built Codex binary is missing. Run build.sh first." >&2
  exit 1
fi

mkdir -p "$artifact_dir"
run_dir="$(mktemp -d "$artifact_dir/fixture-run.XXXXXX")"
schema_dir="$(mktemp -d "$artifact_dir/app-server-schema.XXXXXX")"
printf '%s\n' "$run_dir" > "$artifact_dir/run-path.txt"
printf '%s\n' "$schema_dir" > "$artifact_dir/schema-path.txt"
cp -R "$experiment_dir/fixture" "$run_dir"
git -C "$run_dir" init -q
git -C "$run_dir" add .

python3 -m unittest discover -s "$run_dir" -p 'test_*.py' \
  > "$artifact_dir/test-before.txt" 2>&1 || true

start_epoch="$(date +%s)"
set +e
"$codex_bin" exec \
  --cd "$run_dir" \
  --approve-for-me \
  --ephemeral \
  --json \
  "Read the tiny Python project, run its tests, fix the implementation bug with the smallest reasonable change, and run the tests again. Do not add dependencies or change the test." \
  > "$artifact_dir/events.jsonl" \
  2> "$artifact_dir/runtime-stderr.log"
codex_status="$?"
set -e
end_epoch="$(date +%s)"

printf '%s\n' "$codex_status" > "$artifact_dir/codex-exit-code.txt"
printf '%s\n' "$((end_epoch - start_epoch))" > "$artifact_dir/trace-seconds.txt"

python3 -m unittest discover -s "$run_dir" -p 'test_*.py' \
  > "$artifact_dir/test-after.txt" 2>&1 || true
git -C "$run_dir" diff --no-ext-diff > "$artifact_dir/fixture.diff"

python3 "$experiment_dir/summarize_trace.py" \
  "$artifact_dir/events.jsonl" \
  > "$artifact_dir/event-timeline.md"

"$codex_bin" app-server generate-json-schema --out "$schema_dir" \
  > "$artifact_dir/schema-stdout.log" \
  2> "$artifact_dir/schema-stderr.log"

exit "$codex_status"
