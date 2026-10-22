import json
import shutil
import tempfile
import unittest
from pathlib import Path

from codex_mini.agent import Agent
from codex_mini.events import EventSink
from codex_mini.policy import PolicyError, WorkspacePolicy
from codex_mini.protocol import ModelResponse, ToolCall
from codex_mini.providers import ScriptedClient
from codex_mini.tools import ToolRegistry


CONTRACT_DIR = (
    Path(__file__).resolve().parents[3] / "contracts" / "fix-failing-test"
)


class AgentLoopTest(unittest.TestCase):
    def test_failing_test_patch_passing_test_and_final_message(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            events_path = root / "events.jsonl"
            shutil.copytree(CONTRACT_DIR / "initial", root, dirs_exist_ok=True)
            contract = json.loads((CONTRACT_DIR / "contract.json").read_text())
            responses = []
            for scripted in contract["responses"]:
                responses.append(
                    ModelResponse(
                        text=scripted.get("text"),
                        tool_calls=[
                            ToolCall(call["call_id"], call["name"], call["arguments"])
                            for call in scripted.get("tool_calls", [])
                        ],
                    )
                )
            model = ScriptedClient(
                responses
            )
            agent = Agent(
                model,
                ToolRegistry(WorkspacePolicy(root)),
                EventSink(jsonl_path=events_path),
            )

            final = agent.run(contract["prompt"])

            expected = contract["expected"]
            self.assertEqual(final, expected["final_message"])
            self.assertIn(
                expected["final_file_contains"], (root / "calculator.py").read_text()
            )
            tool_outputs = [step[-1]["output"] for step in model.inputs[1:]]
            self.assertFalse(json.loads(tool_outputs[0])["success"])
            self.assertTrue(json.loads(tool_outputs[1])["success"])
            self.assertTrue(json.loads(tool_outputs[2])["success"])
            events = [json.loads(line) for line in events_path.read_text().splitlines()]
            self.assertEqual([event["type"] for event in events], expected["event_types"])
            self.assertEqual(
                [event["item"]["type"] for event in events if "item" in event],
                expected["item_types"],
            )
            self.assertEqual(len(model.inputs), expected["model_request_count"])
            self.assertEqual(
                [len(history) for history in model.inputs], expected["history_lengths"]
            )
            self.assertEqual(events[4]["item"]["type"], "file_change")

    def test_workspace_and_command_boundaries(self):
        with tempfile.TemporaryDirectory() as temporary:
            temporary_root = Path(temporary)
            workspace = temporary_root / "workspace"
            workspace.mkdir()
            policy = WorkspacePolicy(workspace)
            with self.assertRaises(PolicyError):
                policy.resolve_path("../outside.txt")
            result = ToolRegistry(policy).execute(
                "exec_command", {"argv": ["sh", "-c", "echo unsafe"]}
            )
            self.assertFalse(result.success)
            self.assertIn("not allowed", result.output)
            disguised = ToolRegistry(policy).execute(
                "exec_command", {"argv": ["/tmp/python3", "outside.py"]}
            )
            self.assertFalse(disguised.success)
            self.assertIn("bare executable", disguised.output)
            (temporary_root / "outside.txt").write_text("secret marker", encoding="utf-8")
            (workspace / "link.txt").symlink_to(temporary_root / "outside.txt")
            search = ToolRegistry(policy).execute("search", {"query": "secret marker"})
            self.assertTrue(search.success)
            self.assertEqual(search.metadata["match_count"], 0)


if __name__ == "__main__":
    unittest.main()
