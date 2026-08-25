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
class TrainingProtocolVersion:
    id: str
    major: int
    minor: int
    storage_schema: VersionRange

    @property
    def display_version(self) -> str:
        return f"{self.major}.{self.minor}"


@dataclass(frozen=True)
class RuntimeVersions:
    storage_schema: int
    training_protocol: TrainingProtocolVersion


def _load_versions() -> RuntimeVersions:
    storage = json.loads(
        _metadata_text(
            "storage_schema_version.json",
            Path("crates/spiral-robotics-storage/schema/version.json"),
        )
    )
    protocol = json.loads(
        _metadata_text(
            "training_protocol_version.json",
            Path("crates/spiral-robotics-training/protocol/version.json"),
        )
    )
    _require_keys(storage, {"format_version", "current"}, "storage schema version")
    _require_keys(
        protocol,
        {"format_version", "id", "major", "minor", "storage_schema"},
        "training protocol version",
    )
    if storage["format_version"] != 1:
        raise RuntimeError(f"unsupported storage schema descriptor {storage['format_version']!r}")
    if protocol["format_version"] != 1:
        raise RuntimeError(
            f"unsupported training protocol descriptor {protocol['format_version']!r}"
        )

    supported = protocol["storage_schema"]
    _require_keys(supported, {"minimum", "maximum"}, "training protocol storage_schema")

    current = _positive_int(storage["current"], "storage schema current")
    minimum = _positive_int(supported["minimum"], "storage_schema.minimum")
    maximum = _positive_int(supported["maximum"], "storage_schema.maximum")
    if not minimum <= current <= maximum:
        raise RuntimeError("current storage schema is outside the training runtime support range")
    api_id = protocol["id"]
    if not isinstance(api_id, str) or not api_id:
        raise RuntimeError("training protocol id must be a non-empty string")
    major = _positive_int(protocol["major"], "training protocol major")
    minor = _non_negative_int(protocol["minor"], "training protocol minor")
    return RuntimeVersions(
        storage_schema=current,
        training_protocol=TrainingProtocolVersion(
            id=api_id,
            major=major,
            minor=minor,
            storage_schema=VersionRange(minimum=minimum, maximum=maximum),
        ),
    )


def _metadata_text(packaged_name: str, source_path: Path) -> str:
    packaged = files(__package__).joinpath(packaged_name)
    if packaged.is_file():
        return packaged.read_text(encoding="utf-8")
    source = Path(__file__).resolve().parents[3] / source_path
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


VERSIONS = _load_versions()
