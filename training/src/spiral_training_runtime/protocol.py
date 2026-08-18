from __future__ import annotations

import json
from dataclasses import dataclass
from importlib.resources import files
from pathlib import Path
from typing import Any


@dataclass(frozen=True)
class VersionRange:
    minimum: int
    maximum: int


@dataclass(frozen=True)
class TrainingApiBoundary:
    id: str
    major: int
    minor: int
    spiral_schema: VersionRange

    @property
    def display_version(self) -> str:
        return f"{self.major}.{self.minor}"


@dataclass(frozen=True)
class Boundaries:
    spiral_schema: int
    training_api: TrainingApiBoundary


def _load_boundaries() -> Boundaries:
    descriptor = json.loads(_descriptor_text())
    _require_keys(descriptor, {"format_version", "spiral_schema", "training_api"}, "boundary")
    if descriptor["format_version"] != 1:
        raise RuntimeError(f"unsupported boundary descriptor {descriptor['format_version']!r}")

    spiral = descriptor["spiral_schema"]
    training = descriptor["training_api"]
    supported = training["spiral_schema"]
    _require_keys(spiral, {"current"}, "spiral_schema")
    _require_keys(training, {"id", "major", "minor", "spiral_schema"}, "training_api")
    _require_keys(supported, {"minimum", "maximum"}, "training_api.spiral_schema")

    current = _positive_int(spiral["current"], "spiral_schema.current")
    minimum = _positive_int(supported["minimum"], "training_api.spiral_schema.minimum")
    maximum = _positive_int(supported["maximum"], "training_api.spiral_schema.maximum")
    if not minimum <= current <= maximum:
        raise RuntimeError("current Spiral schema is outside the training runtime support range")
    api_id = training["id"]
    if not isinstance(api_id, str) or not api_id:
        raise RuntimeError("training_api.id must be a non-empty string")
    major = _positive_int(training["major"], "training_api.major")
    minor = _non_negative_int(training["minor"], "training_api.minor")
    return Boundaries(
        spiral_schema=current,
        training_api=TrainingApiBoundary(
            id=api_id,
            major=major,
            minor=minor,
            spiral_schema=VersionRange(minimum=minimum, maximum=maximum),
        ),
    )


def _descriptor_text() -> str:
    packaged = files(__package__).joinpath("boundaries.json")
    if packaged.is_file():
        return packaged.read_text(encoding="utf-8")
    source = Path(__file__).resolve().parents[3] / "contracts" / "boundaries.json"
    return source.read_text(encoding="utf-8")


def _require_keys(value: Any, expected: set[str], label: str) -> None:
    if not isinstance(value, dict) or set(value) != expected:
        actual = sorted(value) if isinstance(value, dict) else type(value).__name__
        raise RuntimeError(f"{label} fields are {actual!r}; expected {sorted(expected)!r}")


def _positive_int(value: Any, label: str) -> int:
    result = _non_negative_int(value, label)
    if result == 0:
        raise RuntimeError(f"{label} must be positive")
    return result


def _non_negative_int(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise RuntimeError(f"{label} must be a non-negative integer")
    return value


BOUNDARIES = _load_boundaries()
