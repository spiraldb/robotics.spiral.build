"""The progress job's environment is Score progress's: whatever warp-progress needs at runtime is
locked here at the version warp-progress locks it, so training embeds pictures with the scorer's
exact stack."""

from __future__ import annotations

import tomllib
from pathlib import Path

PROGRESS = Path(__file__).resolve().parents[1]
SCORER = PROGRESS.parent / "warp_progress"


class Lock:
    """A `uv.lock`'s packages, by name."""

    def __init__(self, path: Path) -> None:
        self._packages: dict[str, list[dict]] = {}
        for package in tomllib.loads(path.read_text())["package"]:
            self._packages.setdefault(package["name"], []).append(package)

    def versions(self, name: str) -> set[str]:
        return {package["version"] for package in self._packages.get(name, [])}

    def runtime_closure(self, root: str) -> set[str]:
        """Every package `root` needs at runtime, with the extras it asks for; not its dev group."""
        needed: set[str] = set()
        pending = [(root, [])]
        while pending:
            name, extras = pending.pop()
            for package in self._packages[name]:
                requirements = list(package.get("dependencies", []))
                for extra in extras:
                    requirements += package.get("optional-dependencies", {}).get(extra, [])
                for requirement in requirements:
                    if requirement["name"] not in needed:
                        needed.add(requirement["name"])
                        pending.append((requirement["name"], requirement.get("extra", [])))
        return needed


def test_the_progress_job_locks_score_progresss_stack() -> None:
    scorer = Lock(SCORER / "uv.lock")
    job = Lock(PROGRESS / "uv.lock")
    stack = scorer.runtime_closure("warp-progress")

    differing = {
        name: (scorer.versions(name), job.versions(name))
        for name in sorted(stack)
        if scorer.versions(name) != job.versions(name)
    }

    assert {"torch", "numpy", "transformers", "av", "nvidia-cudnn-cu13"} <= stack
    assert differing == {}
    assert "lerobot" not in job.runtime_closure("spiral-training-progress")
