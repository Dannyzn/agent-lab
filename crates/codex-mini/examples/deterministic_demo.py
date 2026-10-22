import tempfile
from pathlib import Path

from codex_mini.agent import Agent
from codex_mini.events import stdout_sink
from codex_mini.policy import WorkspacePolicy
from codex_mini.protocol import ModelResponse, ToolCall
from codex_mini.providers import ScriptedClient
from codex_mini.tools import ToolRegistry


PATCH = """*** Begin Patch
*** Update File: calculator.py
@@
-return 2 - 3
+return 2 + 3
*** End Patch"""


with tempfile.TemporaryDirectory() as temporary:
    workspace = Path(temporary)
    (workspace / "calculator.py").write_text("return 2 - 3\n", encoding="utf-8")
    model = ScriptedClient(
        [
            ModelResponse(
                tool_calls=[ToolCall("read", "read_file", {"path": "calculator.py"})]
            ),
            ModelResponse(
                tool_calls=[ToolCall("fix", "apply_patch", {"patch": PATCH})]
            ),
            ModelResponse(text="Fixed calculator.py."),
        ]
    )
    Agent(
        model,
        ToolRegistry(WorkspacePolicy(workspace)),
        stdout_sink(workspace / "events.jsonl"),
    ).run("Fix the arithmetic bug")
    print((workspace / "calculator.py").read_text(encoding="utf-8"), end="")
