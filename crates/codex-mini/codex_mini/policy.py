from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Sequence


class PolicyError(RuntimeError):
    pass


@dataclass(frozen=True)
class WorkspacePolicy:
    root: Path
    allowed_commands: frozenset = frozenset(
        {"python", "python3", "pytest", "git", "rg", "ls", "cat"}
    )

    def __post_init__(self) -> None:
        object.__setattr__(self, "root", self.root.resolve())

    def resolve_path(self, requested: str) -> Path:
        if ".." in Path(requested).parts:
            raise PolicyError("parent path components are not allowed: {}".format(requested))
        candidate = (self.root / requested).resolve()
        try:
            candidate.relative_to(self.root)
        except ValueError as error:
            raise PolicyError("path escapes workspace: {}".format(requested)) from error
        return candidate

    def validate_command(self, argv: Sequence[str]) -> None:
        if not argv:
            raise PolicyError("command argv cannot be empty")
        executable = Path(argv[0]).name
        if argv[0] != executable:
            raise PolicyError("command must use an allow-listed bare executable name")
        if executable not in self.allowed_commands:
            raise PolicyError("command is not allowed: {}".format(executable))

    @classmethod
    def with_commands(cls, root: Path, commands: Iterable[str]) -> "WorkspacePolicy":
        return cls(root=root, allowed_commands=frozenset(commands))
