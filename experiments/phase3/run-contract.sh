#!/usr/bin/env bash
set -euo pipefail
lab_root="$(cd "$(dirname "$0")/../.." && pwd)"
export PYTHONDONTWRITEBYTECODE=1
python3 "$lab_root/experiments/phase3/test_cli.py"
