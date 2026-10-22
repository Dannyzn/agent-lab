#!/usr/bin/env bash
set -euo pipefail

experiment_dir="$(cd "$(dirname "$0")" && pwd)"
lab_root="$(cd "$experiment_dir/../.." && pwd)"
contract_dir="$lab_root/contracts/fix-failing-test"
artifact_dir="$experiment_dir/artifacts"

export CARGO_HOME="$lab_root/.tools/cargo"
export RUSTUP_HOME="$lab_root/.tools/rustup"
export RUSTUP_TOOLCHAIN=stable
export PATH="$CARGO_HOME/bin:$PATH"
export PYTHONDONTWRITEBYTECODE=1

mkdir -p "$artifact_dir"
: > "$artifact_dir/events.jsonl"
run_dir="$(mktemp -d "$artifact_dir/run.XXXXXX")"
server_dir="$(mktemp -d "$artifact_dir/server.XXXXXX")"
cp -R "$contract_dir/initial/." "$run_dir"
git -C "$run_dir" init -q
git -C "$run_dir" add .

python3 -m unittest discover -s "$run_dir" -p 'test_*.py' \
  > "$artifact_dir/test-before.txt" 2>&1 || true

python3 "$lab_root/contracts/mock_responses_server.py" \
  --contract "$contract_dir/contract.json" \
  --port-file "$server_dir/port.txt" \
  --request-log "$server_dir/requests.jsonl" &
server_pid="$!"
trap 'kill "$server_pid" 2>/dev/null || true' EXIT

for _ in 1 2 3 4 5 6 7 8 9 10; do
  [[ -s "$server_dir/port.txt" ]] && break
  sleep 0.1
done
[[ -s "$server_dir/port.txt" ]]

port="$(cat "$server_dir/port.txt")"
export OPENAI_API_KEY=contract-test-key
"$lab_root/target/debug/our-agent" \
  --workspace "$run_dir" \
  --base-url "http://127.0.0.1:$port/v1" \
  --model contract-model \
  --json \
  --events "$artifact_dir/events.jsonl" \
  "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["prompt"])' "$contract_dir/contract.json")" \
  > "$artifact_dir/stdout.jsonl" \
  2> "$artifact_dir/stderr.log"

python3 -m unittest discover -s "$run_dir" -p 'test_*.py' \
  > "$artifact_dir/test-after.txt" 2>&1
git -C "$run_dir" diff --no-ext-diff > "$artifact_dir/fixture.diff"
python3 "$experiment_dir/verify_contract.py" \
  --contract "$contract_dir/contract.json" \
  --events "$artifact_dir/events.jsonl" \
  --stdout "$artifact_dir/stdout.jsonl" \
  --requests "$server_dir/requests.jsonl" \
  --workspace "$run_dir" \
  | tee "$artifact_dir/verification.txt"

kill "$server_pid" 2>/dev/null || true
wait "$server_pid" 2>/dev/null || true

human_run_dir="$(mktemp -d "$artifact_dir/human-run.XXXXXX")"
human_server_dir="$(mktemp -d "$artifact_dir/human-server.XXXXXX")"
cp -R "$contract_dir/initial/." "$human_run_dir"
git -C "$human_run_dir" init -q
git -C "$human_run_dir" add .
: > "$artifact_dir/human-events.jsonl"

python3 "$lab_root/contracts/mock_responses_server.py" \
  --contract "$contract_dir/contract.json" \
  --port-file "$human_server_dir/port.txt" \
  --request-log "$human_server_dir/requests.jsonl" &
server_pid="$!"
for _ in 1 2 3 4 5 6 7 8 9 10; do
  [[ -s "$human_server_dir/port.txt" ]] && break
  sleep 0.1
done
[[ -s "$human_server_dir/port.txt" ]]
human_port="$(cat "$human_server_dir/port.txt")"

"$lab_root/target/debug/our-agent" \
  --workspace "$human_run_dir" \
  --base-url "http://127.0.0.1:$human_port/v1" \
  --model contract-model \
  --events "$artifact_dir/human-events.jsonl" \
  "$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["prompt"])' "$contract_dir/contract.json")" \
  > "$artifact_dir/human-stdout.txt" \
  2> "$artifact_dir/human-stderr.log"

python3 "$experiment_dir/verify_contract.py" \
  --contract "$contract_dir/contract.json" \
  --events "$artifact_dir/human-events.jsonl" \
  --requests "$human_server_dir/requests.jsonl" \
  --workspace "$human_run_dir" \
  > "$artifact_dir/human-verification.txt"
python3 - "$contract_dir/contract.json" "$artifact_dir/human-stdout.txt" <<'PY'
import json, sys
contract = json.load(open(sys.argv[1]))
output = open(sys.argv[2]).read()
assert contract["expected"]["final_message"] in output
assert "Diff:" in output
assert "-    return left - right" in output
assert "+    return left + right" in output
print("human output and git diff: ok")
PY

printf '%s\n' "$run_dir" > "$artifact_dir/run-path.txt"
printf '%s\n' "$server_dir" > "$artifact_dir/server-path.txt"
printf '%s\n' "$human_run_dir" > "$artifact_dir/human-run-path.txt"
printf '%s\n' "$human_server_dir" > "$artifact_dir/human-server-path.txt"
