from __future__ import annotations

import json
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, Optional, TextIO


class EventSink:
    def __init__(
        self,
        jsonl_path: Optional[Path] = None,
        stream: Optional[TextIO] = None,
    ) -> None:
        self.jsonl_path = jsonl_path
        self.stream = stream
        if jsonl_path is not None:
            jsonl_path.parent.mkdir(parents=True, exist_ok=True)

    def emit(self, event_type: str, **payload: Any) -> Dict[str, Any]:
        event = {
            "type": event_type,
            "timestamp": datetime.now(timezone.utc).isoformat(),
            **payload,
        }
        encoded = json.dumps(event, ensure_ascii=False, separators=(",", ":"))
        if self.jsonl_path is not None:
            with self.jsonl_path.open("a", encoding="utf-8") as output:
                output.write(encoded + "\n")
        if self.stream is not None:
            self.stream.write(encoded + "\n")
            self.stream.flush()
        return event


def stdout_sink(jsonl_path: Optional[Path] = None) -> EventSink:
    return EventSink(jsonl_path=jsonl_path, stream=sys.stdout)
