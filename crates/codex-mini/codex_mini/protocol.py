from __future__ import annotations

from dataclasses import dataclass, field
from typing import Any, Dict, List, Optional, Protocol, Sequence


@dataclass
class ToolCall:
    call_id: str
    name: str
    arguments: Dict[str, Any]


@dataclass
class ModelResponse:
    response_id: Optional[str] = None
    text: Optional[str] = None
    tool_calls: List[ToolCall] = field(default_factory=list)


class ModelClient(Protocol):
    def complete(
        self,
        input_history: Sequence[Dict[str, Any]],
        tool_specs: Sequence[Dict[str, Any]],
    ) -> ModelResponse:
        ...
