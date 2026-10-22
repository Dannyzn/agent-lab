from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from typing import Any, Dict, List, Sequence

from .protocol import ModelResponse, ToolCall


DEFAULT_INSTRUCTIONS = """You are a small coding agent. Work inside the provided workspace.
Use tools to inspect, edit, and verify the project. Prefer the smallest correct change.
When the task is complete, return a concise final message."""


class OpenAIResponsesClient:
    def __init__(
        self,
        model: str,
        api_key: str = "",
        base_url: str = "https://api.openai.com/v1",
        instructions: str = DEFAULT_INSTRUCTIONS,
        timeout_seconds: int = 120,
    ) -> None:
        self.model = model
        self.api_key = api_key or os.environ.get("OPENAI_API_KEY", "")
        if not self.api_key:
            raise ValueError("OPENAI_API_KEY is required")
        self.url = base_url.rstrip("/") + "/responses"
        self.instructions = instructions
        self.timeout_seconds = timeout_seconds
    def complete(
        self,
        input_history: Sequence[Dict[str, Any]],
        tool_specs: Sequence[Dict[str, Any]],
    ) -> ModelResponse:
        payload: Dict[str, Any] = {
            "model": self.model,
            "instructions": self.instructions,
            "input": list(input_history),
            "tools": list(tool_specs),
        }
        request = urllib.request.Request(
            self.url,
            data=json.dumps(payload).encode("utf-8"),
            headers={
                "Authorization": "Bearer {}".format(self.api_key),
                "Content-Type": "application/json",
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout_seconds) as response:
                body = json.loads(response.read().decode("utf-8"))
        except urllib.error.HTTPError as error:
            detail = error.read().decode("utf-8", errors="replace")
            raise RuntimeError("Responses API returned {}: {}".format(error.code, detail))
        calls: List[ToolCall] = []
        text_parts = []
        for item in body.get("output", []):
            if item.get("type") == "function_call":
                calls.append(
                    ToolCall(
                        call_id=item["call_id"],
                        name=item["name"],
                        arguments=json.loads(item.get("arguments") or "{}"),
                    )
                )
            if item.get("type") == "message":
                for content in item.get("content", []):
                    if content.get("type") == "output_text":
                        text_parts.append(content.get("text", ""))
        return ModelResponse(
            response_id=body.get("id"),
            text="".join(text_parts) if text_parts else None,
            tool_calls=calls,
        )


class ScriptedClient:
    def __init__(self, responses: Sequence[ModelResponse]) -> None:
        self.responses = list(responses)
        self.index = 0
        self.inputs: List[List[Dict[str, Any]]] = []

    def complete(
        self,
        input_history: Sequence[Dict[str, Any]],
        tool_specs: Sequence[Dict[str, Any]],
    ) -> ModelResponse:
        self.inputs.append(list(input_history))
        if self.index >= len(self.responses):
            raise RuntimeError("scripted model has no response at step {}".format(self.index))
        response = self.responses[self.index]
        self.index += 1
        return response
