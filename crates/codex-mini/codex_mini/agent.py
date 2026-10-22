from __future__ import annotations

import json
import uuid
from typing import Any, Dict, List, Optional

from .events import EventSink
from .protocol import ModelClient, ModelResponse, ToolCall
from .tools import ToolRegistry


class Agent:
    def __init__(
        self,
        model: ModelClient,
        tools: ToolRegistry,
        events: Optional[EventSink] = None,
        max_steps: int = 16,
    ) -> None:
        self.model = model
        self.tools = tools
        self.events = events or EventSink()
        self.max_steps = max_steps

    def run(self, prompt: str) -> str:
        thread_id = str(uuid.uuid4())
        turn_id = str(uuid.uuid4())
        self.events.emit("thread.started", thread_id=thread_id)
        self.events.emit("turn.started", thread_id=thread_id, turn_id=turn_id)
        input_history: List[Dict[str, Any]] = [
            {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": prompt}],
            }
        ]
        try:
            for _ in range(self.max_steps):
                response = self.model.complete(input_history, self.tools.specs)
                if response.tool_calls:
                    for call in response.tool_calls:
                        input_history.append(
                            {
                                "type": "function_call",
                                "call_id": call.call_id,
                                "name": call.name,
                                "arguments": json.dumps(call.arguments),
                            }
                        )
                        input_history.append(
                            self._execute_call(thread_id, turn_id, call)
                        )
                    continue
                if response.text is None:
                    raise RuntimeError("model returned neither tool calls nor final text")
                item = {
                    "id": str(uuid.uuid4()),
                    "type": "agent_message",
                    "text": response.text,
                }
                self.events.emit(
                    "item.completed", thread_id=thread_id, turn_id=turn_id, item=item
                )
                self.events.emit(
                    "turn.completed", thread_id=thread_id, turn_id=turn_id
                )
                return response.text
            raise RuntimeError("agent exceeded max_steps={}".format(self.max_steps))
        except Exception as error:
            self.events.emit(
                "turn.failed",
                thread_id=thread_id,
                turn_id=turn_id,
                error={"message": str(error), "type": type(error).__name__},
            )
            raise

    def _execute_call(
        self, thread_id: str, turn_id: str, call: ToolCall
    ) -> Dict[str, Any]:
        item_type = {
            "exec_command": "command_execution",
            "apply_patch": "file_change",
        }.get(call.name, "tool_call")
        item = {
            "id": call.call_id,
            "type": item_type,
            "name": call.name,
            "arguments": call.arguments,
            "status": "in_progress",
        }
        self.events.emit(
            "item.started", thread_id=thread_id, turn_id=turn_id, item=item
        )
        result = self.tools.execute(call.name, call.arguments)
        completed_item = {
            **item,
            "status": "completed" if result.success else "failed",
            "result": {
                "success": result.success,
                "output": result.output,
                **result.metadata,
            },
        }
        self.events.emit(
            "item.completed",
            thread_id=thread_id,
            turn_id=turn_id,
            item=completed_item,
        )
        return {
            "type": "function_call_output",
            "call_id": call.call_id,
            "output": result.model_output(),
        }
