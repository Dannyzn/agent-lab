from __future__ import annotations

import argparse
from pathlib import Path
from typing import Optional, Sequence

from .agent import Agent
from .events import stdout_sink
from .policy import WorkspacePolicy
from .providers import OpenAIResponsesClient
from .tools import ToolRegistry


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(prog="codex-mini")
    parser.add_argument("prompt")
    parser.add_argument("--workspace", type=Path, default=Path.cwd())
    parser.add_argument("--model", default="gpt-5.5")
    parser.add_argument("--base-url", default="https://api.openai.com/v1")
    parser.add_argument("--events", type=Path)
    parser.add_argument(
        "--allow-command",
        action="append",
        dest="allowed_commands",
        help="Replace the default executable allow-list; repeat for each command.",
    )
    args = parser.parse_args(argv)
    if args.allowed_commands:
        policy = WorkspacePolicy.with_commands(args.workspace, args.allowed_commands)
    else:
        policy = WorkspacePolicy(args.workspace)
    model = OpenAIResponsesClient(model=args.model, base_url=args.base_url)
    agent = Agent(model, ToolRegistry(policy), stdout_sink(args.events))
    agent.run(args.prompt)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
