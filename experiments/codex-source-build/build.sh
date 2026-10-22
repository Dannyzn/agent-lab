#!/usr/bin/env bash
set -euo pipefail

experiment_dir="$(cd "$(dirname "$0")" && pwd)"
lab_root="$(cd "$experiment_dir/../.." && pwd)"
source_root="$lab_root/upstream/codex"
tool_root="$lab_root/.tools"
artifact_dir="$experiment_dir/artifacts"

export CARGO_HOME="$tool_root/cargo"
export RUSTUP_HOME="$tool_root/rustup"
export RUSTUP_TOOLCHAIN=stable
export PATH="$CARGO_HOME/bin:$PATH"
export CARGO_NET_RETRY=10
export CARGO_NET_GIT_FETCH_WITH_CLI=true
export CARGO_HTTP_TIMEOUT=30
export CARGO_HTTP_MULTIPLEXING=false
export CARGO_HTTP_LOW_SPEED_LIMIT=1

mkdir -p "$artifact_dir"

if [[ ! -f "$source_root/codex-rs/Cargo.toml" ]]; then
  echo "Codex source snapshot is missing at $source_root" >&2
  exit 1
fi

expected_commit="$(tr -d '[:space:]' < "$experiment_dir/SOURCE_COMMIT")"
printf '%s\n' "$expected_commit" > "$artifact_dir/source-commit.txt"
python3 "$experiment_dir/prepare_local_deps.py"
cp "$experiment_dir/LOCAL_DEPENDENCIES" "$artifact_dir/local-dependencies.txt"
rustc --version | tee "$artifact_dir/rustc-version.txt"
cargo --version | tee "$artifact_dir/cargo-version.txt"

start_epoch="$(date +%s)"
(
  cd "$source_root/codex-rs"
  cargo build --locked --bin codex
) 2>&1 | tee "$artifact_dir/build.log"
end_epoch="$(date +%s)"

printf '%s\n' "$((end_epoch - start_epoch))" > "$artifact_dir/build-seconds.txt"
"$source_root/codex-rs/target/debug/codex" --version | tee "$artifact_dir/codex-version.txt"
