#!/usr/bin/env python3
"""Convert `codex exec --json` output into a compact, reviewable timeline."""

from __future__ import annotations

import json
import sys
from collections import Counter
from pathlib import Path
from typing import Any


def compact(value: Any, limit: int = 140) -> str:
    if value is None:
        return ""
    if isinstance(value, (dict, list)):
        text = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    else:
        text = str(value)
    text = " ".join(text.split())
    return text if len(text) <= limit else text[: limit - 1] + "…"


def event_detail(event: dict[str, Any]) -> str:
    item = event.get("item")
    if isinstance(item, dict):
        item_type = item.get("type", "item")
        parts = [str(item_type)]
        for key in ("status", "command", "exit_code", "text"):
            if key in item and item[key] not in (None, ""):
                parts.append(f"{key}={compact(item[key])}")
        changes = item.get("changes")
        if isinstance(changes, list):
            paths = [change.get("path") for change in changes if isinstance(change, dict)]
            parts.append(f"paths={compact([path for path in paths if path])}")
        return "; ".join(parts)

    for key in ("thread_id", "turn_id", "status", "message", "error"):
        if key in event and event[key] not in (None, ""):
            return f"{key}={compact(event[key])}"
    return ""


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {Path(sys.argv[0]).name} EVENTS.jsonl", file=sys.stderr)
        return 2

    source = Path(sys.argv[1])
    events: list[dict[str, Any]] = []
    parse_errors: list[tuple[int, str]] = []
    for line_number, line in enumerate(source.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError as error:
            parse_errors.append((line_number, str(error)))
            continue
        if isinstance(value, dict):
            events.append(value)

    counts = Counter(str(event.get("type", "<missing>")) for event in events)
    print("# Codex event timeline")
    print()
    print(f"- Raw events: {len(events)}")
    print(f"- Parse errors: {len(parse_errors)}")
    print("- Event counts: " + ", ".join(f"`{name}`={count}" for name, count in sorted(counts.items())))
    print()
    print("| # | Event | Detail |")
    print("|---:|---|---|")
    for index, event in enumerate(events, 1):
        event_type = compact(event.get("type", "<missing>"), 80).replace("|", "\\|")
        detail = event_detail(event).replace("|", "\\|")
        print(f"| {index} | `{event_type}` | {detail} |")

    if parse_errors:
        print()
        print("## Parse errors")
        print()
        for line_number, error in parse_errors:
            print(f"- Line {line_number}: {error}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
