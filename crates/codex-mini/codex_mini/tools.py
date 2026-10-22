from __future__ import annotations

import json
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Callable, Dict, List, Mapping, Sequence

from .policy import PolicyError, WorkspacePolicy


@dataclass
class ToolResult:
    success: bool
    output: str
    metadata: Dict[str, Any]

    def model_output(self) -> str:
        return json.dumps(
            {"success": self.success, "output": self.output, **self.metadata},
            ensure_ascii=False,
        )


ToolHandler = Callable[[Mapping[str, Any]], ToolResult]


class ToolRegistry:
    def __init__(self, policy: WorkspacePolicy) -> None:
        self.policy = policy
        self._handlers: Dict[str, ToolHandler] = {
            "read_file": self._read_file,
            "search": self._search,
            "apply_patch": self._apply_patch,
            "exec_command": self._exec_command,
        }

    @property
    def specs(self) -> List[Dict[str, Any]]:
        return [
            _spec(
                "read_file",
                "Read a UTF-8 text file inside the workspace.",
                {"path": {"type": "string"}},
                ["path"],
            ),
            _spec(
                "search",
                "Search UTF-8 workspace files for a literal string.",
                {
                    "query": {"type": "string"},
                    "path": {"type": "string", "default": "."},
                },
                ["query"],
            ),
            _spec(
                "apply_patch",
                "Apply a Codex-style Begin Patch containing Update File sections.",
                {"patch": {"type": "string"}},
                ["patch"],
            ),
            _spec(
                "exec_command",
                "Run an allow-listed argv command in the workspace without a shell.",
                {
                    "argv": {"type": "array", "items": {"type": "string"}},
                    "timeout_ms": {"type": "integer", "minimum": 1, "maximum": 120000},
                },
                ["argv"],
            ),
        ]

    def execute(self, name: str, arguments: Mapping[str, Any]) -> ToolResult:
        handler = self._handlers.get(name)
        if handler is None:
            return ToolResult(False, "unknown tool: {}".format(name), {})
        try:
            return handler(arguments)
        except (OSError, PolicyError, ValueError, TypeError) as error:
            return ToolResult(False, str(error), {"error_type": type(error).__name__})

    def _read_file(self, arguments: Mapping[str, Any]) -> ToolResult:
        path = self.policy.resolve_path(_string(arguments, "path"))
        content = path.read_text(encoding="utf-8")
        return ToolResult(True, content, {"path": str(path.relative_to(self.policy.root))})

    def _search(self, arguments: Mapping[str, Any]) -> ToolResult:
        query = _string(arguments, "query")
        root = self.policy.resolve_path(str(arguments.get("path", ".")))
        files = [root] if root.is_file() else sorted(root.rglob("*"))
        matches = []
        for path in files:
            if not path.is_file() or ".git" in path.parts:
                continue
            try:
                resolved = path.resolve()
                resolved.relative_to(self.policy.root)
                for line_number, line in enumerate(
                    resolved.read_text(encoding="utf-8").splitlines(), 1
                ):
                    if query in line:
                        relative = resolved.relative_to(self.policy.root)
                        matches.append("{}:{}:{}".format(relative, line_number, line))
            except (UnicodeDecodeError, ValueError):
                continue
        return ToolResult(True, "\n".join(matches), {"match_count": len(matches)})

    def _apply_patch(self, arguments: Mapping[str, Any]) -> ToolResult:
        patch = _string(arguments, "patch")
        changed = apply_update_patch(self.policy, patch)
        return ToolResult(True, "updated {}".format(", ".join(changed)), {"files": changed})

    def _exec_command(self, arguments: Mapping[str, Any]) -> ToolResult:
        raw_argv = arguments.get("argv")
        if not isinstance(raw_argv, list) or not all(isinstance(value, str) for value in raw_argv):
            raise TypeError("argv must be a list of strings")
        argv = list(raw_argv)
        self.policy.validate_command(argv)
        timeout_ms = int(arguments.get("timeout_ms", 30000))
        if timeout_ms < 1 or timeout_ms > 120000:
            raise ValueError("timeout_ms must be between 1 and 120000")
        started = time.monotonic()
        try:
            completed = subprocess.run(
                argv,
                cwd=str(self.policy.root),
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                timeout=timeout_ms / 1000,
            )
            elapsed_ms = round((time.monotonic() - started) * 1000)
            output = completed.stdout
            if completed.stderr:
                output += ("\n" if output else "") + completed.stderr
            return ToolResult(
                completed.returncode == 0,
                output.rstrip(),
                {"exit_code": completed.returncode, "elapsed_ms": elapsed_ms},
            )
        except subprocess.TimeoutExpired as error:
            return ToolResult(
                False,
                "command timed out",
                {"exit_code": None, "elapsed_ms": timeout_ms, "timeout": True},
            )


def apply_update_patch(policy: WorkspacePolicy, patch: str) -> List[str]:
    lines = patch.splitlines()
    if not lines or lines[0] != "*** Begin Patch" or lines[-1] != "*** End Patch":
        raise ValueError("patch must use Begin Patch / End Patch markers")
    index = 1
    changed = []
    while index < len(lines) - 1:
        header = lines[index]
        if not header.startswith("*** Update File: "):
            raise ValueError("only Update File patches are supported")
        relative = header[len("*** Update File: ") :]
        path = policy.resolve_path(relative)
        index += 1
        original = path.read_text(encoding="utf-8")
        while index < len(lines) - 1 and lines[index].startswith("@@"):
            index += 1
            old_lines = []
            new_lines = []
            while index < len(lines) - 1 and not lines[index].startswith("@@") and not lines[index].startswith("*** Update File: "):
                line = lines[index]
                if not line or line[0] not in " +-":
                    raise ValueError("invalid patch line: {}".format(line))
                if line[0] in " -":
                    old_lines.append(line[1:])
                if line[0] in " +":
                    new_lines.append(line[1:])
                index += 1
            old = "\n".join(old_lines)
            new = "\n".join(new_lines)
            if old not in original:
                raise ValueError("patch context not found in {}".format(relative))
            original = original.replace(old, new, 1)
        path.write_text(original, encoding="utf-8")
        changed.append(relative)
    return changed


def _spec(
    name: str,
    description: str,
    properties: Dict[str, Any],
    required: Sequence[str],
) -> Dict[str, Any]:
    return {
        "type": "function",
        "name": name,
        "description": description,
        "parameters": {
            "type": "object",
            "properties": properties,
            "required": list(required),
            "additionalProperties": False,
        },
    }


def _string(arguments: Mapping[str, Any], name: str) -> str:
    value = arguments.get(name)
    if not isinstance(value, str):
        raise TypeError("{} must be a string".format(name))
    return value
