#!/usr/bin/env bash
set -euo pipefail

experiment_dir="$(cd "$(dirname "$0")" && pwd)"
lab_root="$(cd "$experiment_dir/../.." && pwd)"
codex_bin="${CODEX_BIN_OVERRIDE:-$lab_root/upstream/codex/codex-rs/target/debug/codex}"
artifact_dir="$experiment_dir/artifacts"

export PYTHONDONTWRITEBYTECODE=1

if [[ ! -x "$codex_bin" ]]; then
  echo "Source-built Codex binary is missing. Run build.sh first." >&2
  exit 1
fi

mkdir -p "$artifact_dir"
run_dir="$(mktemp -d "$artifact_dir/local-e2e-run.XXXXXX")"
server_dir="$(mktemp -d "$artifact_dir/local-e2e-server.XXXXXX")"
codex_home="$(mktemp -d "$artifact_dir/local-e2e-home.XXXXXX")"
schema_dir="$(mktemp -d "$artifact_dir/app-server-schema.XXXXXX")"
port_file="$server_dir/port.txt"
request_log="$server_dir/requests.jsonl"
printf '%s\n' "$run_dir" > "$artifact_dir/local-e2e-run-path.txt"
printf '%s\n' "$server_dir" > "$artifact_dir/local-e2e-server-path.txt"
printf '%s\n' "$schema_dir" > "$artifact_dir/schema-path.txt"

cp -R "$experiment_dir/fixture/." "$run_dir"
git -C "$run_dir" init -q
git -C "$run_dir" add .

python3 -m unittest discover -s "$run_dir" -p 'test_*.py' \
  > "$artifact_dir/local-test-before.txt" 2>&1 || true

python3 "$experiment_dir/mock_responses_server.py" \
  --port-file "$port_file" \
  --request-log "$request_log" \
  > "$server_dir/stdout.log" \
  2> "$server_dir/stderr.log" &
server_pid="$!"
trap 'kill "$server_pid" 2>/dev/null || true' EXIT

for _ in 1 2 3 4 5 6 7 8 9 10; do
  [[ -s "$port_file" ]] && break
  sleep 0.1
done
if [[ ! -s "$port_file" ]]; then
  echo "Local Responses mock did not start." >&2
  exit 1
fi

port="$(cat "$port_file")"
provider="model_providers.local_trace={name=\"local_trace\",base_url=\"http://127.0.0.1:$port/v1\",wire_api=\"responses\",requires_openai_auth=false,supports_websockets=false}"

start_epoch="$(date +%s)"
set +e
CODEX_HOME="$codex_home" "$codex_bin" \
  --dangerously-bypass-approvals-and-sandbox \
  exec \
  --cd "$run_dir" \
  --skip-git-repo-check \
  --ephemeral \
  --json \
  -m gpt-5.5 \
  -c 'model_provider="local_trace"' \
  -c "$provider" \
  "Read the tiny Python project, run its tests, fix the implementation bug with the smallest reasonable change, and run the tests again." \
  > "$artifact_dir/local-events.jsonl" \
  2> "$artifact_dir/local-runtime-stderr.log"
codex_status="$?"
set -e
end_epoch="$(date +%s)"

printf '%s\n' "$codex_status" > "$artifact_dir/local-codex-exit-code.txt"
printf '%s\n' "$((end_epoch - start_epoch))" > "$artifact_dir/local-trace-seconds.txt"
python3 -m unittest discover -s "$run_dir" -p 'test_*.py' \
  > "$artifact_dir/local-test-after.txt" 2>&1 || true
git -C "$run_dir" diff --no-ext-diff > "$artifact_dir/local-fixture.diff"
python3 "$experiment_dir/summarize_trace.py" \
  "$artifact_dir/local-events.jsonl" \
  > "$artifact_dir/local-event-timeline.md"

"$codex_bin" app-server generate-json-schema --out "$schema_dir" \
  > "$artifact_dir/schema-stdout.log" \
  2> "$artifact_dir/schema-stderr.log"

exit "$codex_status"
