"""Canonical Spiral-to-ACT batch boundary.

Python submits finite coordinate manifests. Spiral owns relational lookup,
video frame selection, decode, resize, ImageNet normalization, and conversion
to PyTorch. Encoded media never crosses into Python.
"""

from __future__ import annotations

import bisect
import hashlib
import re
import time
from collections import defaultdict
from dataclasses import dataclass
from fractions import Fraction
from pathlib import Path
from typing import Any, Iterator, Sequence

import numpy as np
import pyarrow as pa
import spiraldb as sp
import torch


IDENTIFIER = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
MAX_MANIFEST_ROWS = 8_192


def _sql_string(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def _clean_query(sql: str) -> str:
    query = sql.strip()
    while query.endswith(";"):
        query = query[:-1].rstrip()
    if not query:
        raise ValueError("the Spiral training query cannot be empty")
    return query


def _column_length(value: Any) -> int:
    if isinstance(value, torch.Tensor):
        return int(value.shape[0])
    return len(value)


def _column_item(value: Any, index: int) -> Any:
    item = value[index]
    if isinstance(item, torch.Tensor) and item.ndim == 0:
        return item.item()
    return item


def _batch_rows(batch: dict[str, Any], required: Sequence[str]) -> Iterator[dict[str, Any]]:
    missing = [name for name in required if name not in batch]
    if missing:
        raise ValueError(f"Spiral relation is missing required columns: {', '.join(missing)}")
    rows = _column_length(batch[required[0]])
    for name in required[1:]:
        if _column_length(batch[name]) != rows:
            raise ValueError(f"Spiral returned inconsistent batch length for {name}")
    for index in range(rows):
        yield {name: _column_item(batch[name], index) for name in required}


def _vector(value: Any, name: str) -> torch.Tensor:
    tensor = (
        value.detach().to(device="cpu", dtype=torch.float32)
        if isinstance(value, torch.Tensor)
        else torch.as_tensor(value, dtype=torch.float32)
    )
    if tensor.ndim != 1 or tensor.numel() == 0:
        raise ValueError(f"{name} must contain a non-empty one-dimensional numeric vector")
    if not torch.isfinite(tensor).all():
        raise ValueError(f"{name} contains a non-finite value")
    return tensor


def _strings(value: Any, name: str) -> tuple[str, ...]:
    if isinstance(value, torch.Tensor):
        value = value.detach().cpu().tolist()
    elif hasattr(value, "tolist") and not isinstance(value, (str, bytes)):
        value = value.tolist()
    if not isinstance(value, (list, tuple)):
        raise ValueError(f"{name} must contain a list of strings")
    result = tuple(str(item) for item in value)
    if not result or any(not item for item in result) or len(set(result)) != len(result):
        raise ValueError(f"{name} must contain unique, non-empty joint names")
    return result


def _robot_type(value: Any, stream_id: str) -> str:
    if value is not None:
        rendered = str(value).strip()
        if rendered:
            return rendered
    # Episodes recorded before robot_type entered the stream dimension used
    # the canonical follower stream ID. Keep that finite legacy bridge here;
    # new and custom stream IDs must carry explicit compatibility metadata.
    if stream_id == "follower":
        return "so101_follower"
    raise ValueError(f"joint stream {stream_id!r} omitted its robot type")


def _validation_episode(episode_id: str) -> bool:
    digest = hashlib.blake2b(episode_id.encode("utf-8"), digest_size=8).digest()
    return int.from_bytes(digest, "little") % 10 == 0


@dataclass(frozen=True)
class Anchor:
    episode_id: str
    stream_id: str
    sample_index: int
    sample_hz: Fraction


@dataclass(frozen=True)
class SampleRange:
    episode_id: str
    stream_id: str
    first_index: int
    last_index: int
    episode_frames: int
    robot_type: str
    sample_hz: Fraction

    @property
    def count(self) -> int:
        return self.last_index - self.first_index + 1


@dataclass(frozen=True)
class StreamContract:
    robot_type: str
    joint_names: tuple[str, ...]


@dataclass(frozen=True)
class VideoSegment:
    episode_id: str
    camera_stream_id: str
    segment_index: int
    timeline_offset_ns: int
    fps: int
    frames: int
    media_relation: str

    @property
    def start_seconds(self) -> Fraction:
        return Fraction(self.timeline_offset_ns, 1_000_000_000)

    @property
    def end_seconds(self) -> Fraction:
        return self.start_seconds + Fraction(self.frames, self.fps)

    def sample_bounds(self, sample_hz: Fraction) -> tuple[int, int]:
        first = _ceil_fraction(self.start_seconds * sample_hz)
        last = _ceil_fraction(self.end_seconds * sample_hz) - 1
        return first, last

    def frame_for(self, sample_index: int, sample_hz: Fraction) -> int | None:
        sample_seconds = Fraction(sample_index, 1) / sample_hz
        if sample_seconds < self.start_seconds or sample_seconds >= self.end_seconds:
            return None
        frame = (sample_seconds - self.start_seconds) * self.fps
        index = frame.numerator // frame.denominator
        return index if 0 <= index < self.frames else None


@dataclass(frozen=True)
class ImageCoordinate:
    batch_index: int
    camera_index: int
    episode_id: str
    camera_stream_id: str
    segment_index: int
    frame_index: int
    media_relation: str


@dataclass(frozen=True)
class FrameGroup:
    group_id: int
    episode_id: str
    camera_stream_id: str
    segment_index: int
    media_relation: str
    frame_indices: tuple[int, ...]
    destinations: tuple[tuple[int, int], ...]


@dataclass(frozen=True)
class NormalizationStats:
    state_mean: torch.Tensor
    state_std: torch.Tensor
    action_mean: torch.Tensor
    action_std: torch.Tensor

    @classmethod
    def normalized_joint_space(cls, state_dim: int, action_dim: int) -> "NormalizationStats":
        return cls(
            state_mean=torch.zeros(state_dim),
            state_std=torch.ones(state_dim),
            action_mean=torch.zeros(action_dim),
            action_std=torch.ones(action_dim),
        )

    def as_checkpoint(self) -> dict[str, torch.Tensor]:
        return {
            "state_mean": self.state_mean,
            "state_std": self.state_std,
            "action_mean": self.action_mean,
            "action_std": self.action_std,
        }


@dataclass
class LoaderMetrics:
    seconds: float
    source_bytes: int
    requested_frames: int


@dataclass
class ActBatch:
    state: torch.Tensor
    images: torch.Tensor
    actions: torch.Tensor
    action_is_pad: torch.Tensor
    metrics: LoaderMetrics


class TrainingCancelled(RuntimeError):
    """Studio requested cancellation at a safe relation boundary."""


def _ceil_fraction(value: Fraction) -> int:
    return -(-value.numerator // value.denominator)


class SpiralActLoader:
    """Prepare validated random ACT mini-batches from Spiral relations."""

    def __init__(self, config: dict[str, Any], device: torch.device) -> None:
        request = config["request"]
        act = request["act"]
        self.base_sql = _clean_query(request["sql"])
        self.state_column = str(request["state_column"])
        self.action_column = str(request["action_column"])
        for name in (self.state_column, self.action_column):
            if IDENTIFIER.fullmatch(name) is None:
                raise ValueError(f"training column {name!r} is not a SQL identifier")
        self.dataset = str(config["spiral_dataset"])
        if IDENTIFIER.fullmatch(self.dataset) is None:
            raise ValueError("the configured Spiral dataset is not a SQL identifier")
        self.camera_streams = tuple(str(value) for value in act["camera_streams"])
        self.chunk_size = int(request["chunk_size"])
        self.image_width = int(act["image_width"])
        self.image_height = int(act["image_height"])
        self.video_decoder = str(act.get("video_decoder", "auto"))
        self.prefetch = 2
        self.device = device
        self.stop_file = Path(config["stop_file"])
        self.db = sp.Spiral()
        self.db.use_project(config["spiral_project"])
        self.train_random = np.random.default_rng(int(request["seed"]))
        self.validation_random = np.random.default_rng(int(request["seed"]) ^ 0xA17C9E3D)

        ranges = self._load_population()
        rates = {item.sample_hz for item in ranges}
        if len(rates) != 1:
            raise ValueError("one ACT run requires one fixed sample rate")
        self.sample_hz = next(iter(rates))
        self.contracts = self._load_stream_contracts(ranges)
        self.video_segments = self._load_video_segments(ranges)
        ranges = self._require_camera_coverage(ranges)
        self.train_ranges = tuple(item for item in ranges if not _validation_episode(item.episode_id))
        self.validation_ranges = tuple(item for item in ranges if _validation_episode(item.episode_id))
        if not self.train_ranges:
            raise ValueError("the query has no training samples with complete synchronized camera coverage")
        self._train_prefix = self._prefix(self.train_ranges)
        self._validation_prefix = self._prefix(self.validation_ranges)
        state_dim, action_dim = self._load_vector_contract(self.train_ranges[0])
        contract = self.contracts[(self.train_ranges[0].episode_id, self.train_ranges[0].stream_id)]
        if state_dim != len(contract.joint_names) or action_dim != len(contract.joint_names):
            raise ValueError("state/action vector widths do not match the persisted joint-name contract")
        self.stats = NormalizationStats.normalized_joint_space(state_dim, action_dim)
        self.joint_names = contract.joint_names
        self.robot_type = contract.robot_type

    @property
    def state_dim(self) -> int:
        return int(self.stats.state_mean.numel())

    @property
    def action_dim(self) -> int:
        return int(self.stats.action_mean.numel())

    @property
    def validation_anchors(self) -> bool:
        return bool(self.validation_ranges)

    def summary(self) -> str:
        return (
            f"{self._range_count(self.train_ranges):,} train anchors, "
            f"{self._range_count(self.validation_ranges):,} validation anchors, "
            f"{len(self.camera_streams)} camera(s)"
        )

    def _check_cancelled(self) -> None:
        if self.stop_file.exists():
            raise TrainingCancelled("training cancellation requested")

    def _arrow_rows(self, relation: Any, required: Sequence[str]) -> Iterator[dict[str, Any]]:
        """Read the small startup/control plane without preparing a Torch stream."""
        self._check_cancelled()
        for batch in relation.to_record_batches(batch_readahead=self.prefetch):
            missing = [name for name in required if name not in batch.schema.names]
            if missing:
                raise ValueError(
                    f"Spiral relation is missing required columns: {', '.join(missing)}"
                )
            for row in batch.to_pylist():
                yield {name: row[name] for name in required}
            self._check_cancelled()

    def _torch_rows(self, relation: Any, required: Sequence[str]) -> Iterator[dict[str, Any]]:
        """Stream per-step numeric and image payloads through ``to_torch()``."""
        self._check_cancelled()
        for batch in relation.to_torch(device="cpu", prefetch=self.prefetch):
            yield from _batch_rows(batch, required)
            self._check_cancelled()

    @staticmethod
    def _range_count(ranges: Sequence[SampleRange]) -> int:
        return sum(item.count for item in ranges)

    @staticmethod
    def _prefix(ranges: Sequence[SampleRange]) -> tuple[int, ...]:
        total = 0
        prefix = []
        for item in ranges:
            total += item.count
            prefix.append(total)
        return tuple(prefix)

    def _load_population(self) -> list[SampleRange]:
        query = f"""
WITH training_rows AS (
{self.base_sql}
)
SELECT episode_id, stream_id,
       MIN(sample_index) AS first_index,
       MAX(sample_index) AS last_index,
       COUNT(*) AS row_count,
       MIN(sample_hz) AS sample_hz_min,
       MAX(sample_hz) AS sample_hz_max,
       MIN(episode_frames) AS episode_frames_min,
       MAX(episode_frames) AS episode_frames_max,
       MIN(robot_type) AS robot_type_min,
       MAX(robot_type) AS robot_type_max
FROM training_rows
GROUP BY episode_id, stream_id
"""
        required = (
            "episode_id",
            "stream_id",
            "first_index",
            "last_index",
            "row_count",
            "sample_hz_min",
            "sample_hz_max",
            "episode_frames_min",
            "episode_frames_max",
            "robot_type_min",
            "robot_type_max",
        )
        result = []
        for row in self._arrow_rows(self.db.sql(query), required):
            first = int(row["first_index"])
            last = int(row["last_index"])
            count = int(row["row_count"])
            sample_hz_min = Fraction(str(float(row["sample_hz_min"])))
            sample_hz_max = Fraction(str(float(row["sample_hz_max"])))
            frames_min = int(row["episode_frames_min"])
            frames_max = int(row["episode_frames_max"])
            key = (str(row["episode_id"]), str(row["stream_id"]))
            robot_min = _robot_type(row["robot_type_min"], key[1])
            robot_max = _robot_type(row["robot_type_max"], key[1])
            if first < 0 or last < first or count != last - first + 1:
                raise ValueError(f"training rows for {key} are duplicated or non-contiguous")
            if frames_min != frames_max or frames_min <= last:
                raise ValueError(f"training rows for {key} have an inconsistent episode frame count")
            if sample_hz_min != sample_hz_max or sample_hz_min <= 0:
                raise ValueError(f"training rows for {key} have an inconsistent sample rate")
            if robot_min != robot_max or not robot_min:
                raise ValueError(f"training rows for {key} mixed or omitted robot semantics")
            result.append(
                SampleRange(
                    key[0],
                    key[1],
                    first,
                    last,
                    frames_min,
                    robot_min,
                    sample_hz_min,
                )
            )
        if not result:
            raise ValueError("the Spiral training query returned no rows")
        return result

    def _load_stream_contracts(
        self, ranges: Sequence[SampleRange]
    ) -> dict[tuple[str, str], StreamContract]:
        query = (
            "SELECT episode_id, stream_id, robot_type, joint_names "
            f"FROM {self.dataset}.episode_streams WHERE modality = 'joint'"
        )
        wanted = {(item.episode_id, item.stream_id): item for item in ranges}
        contracts: dict[tuple[str, str], StreamContract] = {}
        required = ("episode_id", "stream_id", "robot_type", "joint_names")
        for row in self._arrow_rows(self.db.sql(query), required):
            key = (str(row["episode_id"]), str(row["stream_id"]))
            if key not in wanted:
                continue
            contract = StreamContract(
                robot_type=_robot_type(row["robot_type"], key[1]),
                joint_names=_strings(row["joint_names"], "joint_names"),
            )
            if key in contracts:
                raise ValueError(f"duplicate stream contract for {key}")
            if contract.robot_type != wanted[key].robot_type:
                raise ValueError(f"robot type changed between training rows and stream contract for {key}")
            contracts[key] = contract
        missing = wanted.keys() - contracts.keys()
        if missing:
            raise ValueError(f"missing persisted stream contracts for {sorted(missing)[:3]}")
        canonical = next(iter(contracts.values()))
        incompatible = [key for key, value in contracts.items() if value != canonical]
        if incompatible:
            raise ValueError(
                "one ACT run requires one robot type and one exact joint order; "
                f"incompatible stream: {incompatible[0]}"
            )
        return contracts

    def _load_video_segments(
        self, ranges: Sequence[SampleRange]
    ) -> dict[tuple[str, str], tuple[VideoSegment, ...]]:
        cameras = ", ".join(_sql_string(camera) for camera in self.camera_streams)
        query = f"""
SELECT episode_id, stream_id AS camera_stream_id, segment_index,
       timeline_offset_ns, fps, frames, media_relation
FROM {self.dataset}.video_assets
WHERE stream_id IN ({cameras})
"""
        wanted_episodes = {item.episode_id for item in ranges}
        segments: dict[tuple[str, str], list[VideoSegment]] = defaultdict(list)
        required = (
            "episode_id",
            "camera_stream_id",
            "segment_index",
            "timeline_offset_ns",
            "fps",
            "frames",
            "media_relation",
        )
        for row in self._arrow_rows(self.db.sql(query), required):
            episode_id = str(row["episode_id"])
            if episode_id not in wanted_episodes:
                continue
            timeline_offset = row["timeline_offset_ns"]
            if timeline_offset is None:
                raise ValueError(
                    f"camera segment in episode {episode_id!r} has no fixed-FPS timeline offset"
                )
            segment = VideoSegment(
                episode_id=episode_id,
                camera_stream_id=str(row["camera_stream_id"]),
                segment_index=int(row["segment_index"]),
                timeline_offset_ns=int(timeline_offset),
                fps=int(row["fps"]),
                frames=int(row["frames"]),
                media_relation=str(row["media_relation"]),
            )
            if segment.fps <= 0 or segment.frames <= 0 or not segment.media_relation:
                raise ValueError(
                    f"camera segment {(episode_id, segment.camera_stream_id, segment.segment_index)} "
                    "has an invalid fixed-FPS contract"
                )
            segments[(episode_id, segment.camera_stream_id)].append(segment)

        result: dict[tuple[str, str], tuple[VideoSegment, ...]] = {}
        for key, values in segments.items():
            values.sort(key=lambda segment: (segment.timeline_offset_ns, segment.segment_index))
            if len({segment.segment_index for segment in values}) != len(values):
                raise ValueError(f"duplicate camera segment index for {key}")
            for previous, current in zip(values, values[1:]):
                if current.start_seconds < previous.end_seconds:
                    raise ValueError(f"overlapping fixed-FPS camera segments for {key}")
            result[key] = tuple(values)
        return result

    @staticmethod
    def _intersect_intervals(
        left: Sequence[tuple[int, int]], right: Sequence[tuple[int, int]]
    ) -> list[tuple[int, int]]:
        result = []
        left_index = 0
        right_index = 0
        while left_index < len(left) and right_index < len(right):
            first = max(left[left_index][0], right[right_index][0])
            last = min(left[left_index][1], right[right_index][1])
            if first <= last:
                result.append((first, last))
            if left[left_index][1] < right[right_index][1]:
                left_index += 1
            else:
                right_index += 1
        return result

    def _require_camera_coverage(self, ranges: Sequence[SampleRange]) -> list[SampleRange]:
        accepted = []
        for item in ranges:
            intervals = [(item.first_index, item.last_index)]
            for camera in self.camera_streams:
                segments = self.video_segments.get((item.episode_id, camera), ())
                camera_intervals = sorted(
                    segment.sample_bounds(item.sample_hz) for segment in segments
                )
                intervals = self._intersect_intervals(intervals, camera_intervals)
                if not intervals:
                    break
            accepted.extend(
                SampleRange(
                    item.episode_id,
                    item.stream_id,
                    first,
                    last,
                    item.episode_frames,
                    item.robot_type,
                    item.sample_hz,
                )
                for first, last in intervals
            )
        return accepted

    def _load_vector_contract(self, item: SampleRange) -> tuple[int, int]:
        query = f"""
WITH training_rows AS (
{self.base_sql}
)
SELECT {self.state_column} AS state_value,
       {self.action_column} AS action_value
FROM training_rows
WHERE episode_id = {_sql_string(item.episode_id)}
  AND stream_id = {_sql_string(item.stream_id)}
  AND sample_index = {item.first_index}
"""
        rows = list(
            self._arrow_rows(self.db.sql(query), ("state_value", "action_value"))
        )
        if len(rows) != 1:
            raise ValueError("the sampled stream did not resolve to one vector contract row")
        return (
            int(_vector(rows[0]["state_value"], "state_value").numel()),
            int(_vector(rows[0]["action_value"], "action_value").numel()),
        )

    def _sample_anchors(self, validation: bool, batch_size: int) -> list[Anchor]:
        ranges = self.validation_ranges if validation else self.train_ranges
        prefix = self._validation_prefix if validation else self._train_prefix
        if not ranges:
            raise ValueError("the episode split contains no validation anchors")
        random = self.validation_random if validation else self.train_random
        offsets = random.integers(0, prefix[-1], size=batch_size)
        result = []
        for offset_value in offsets:
            offset = int(offset_value)
            range_index = bisect.bisect_right(prefix, offset)
            previous = prefix[range_index - 1] if range_index else 0
            item = ranges[range_index]
            result.append(
                Anchor(
                    item.episode_id,
                    item.stream_id,
                    item.first_index + offset - previous,
                    item.sample_hz,
                )
            )
        return result

    def _action_query(self, anchors: Sequence[Anchor]) -> str:
        requests = ",\n".join(
            f"({index}, {offset}, {_sql_string(anchor.episode_id)}, "
            f"{_sql_string(anchor.stream_id)}, {anchor.sample_index + offset})"
            for index, anchor in enumerate(anchors)
            for offset in range(self.chunk_size)
        )
        return f"""
WITH training_rows AS (
{self.base_sql}
),
requested(request_index, chunk_offset, episode_id, stream_id, sample_index) AS (
    VALUES {requests}
)
SELECT r.request_index, r.chunk_offset,
       j.sample_index IS NOT NULL AS present,
       j.{self.state_column} AS state_value,
       j.{self.action_column} AS action_value
FROM requested AS r
LEFT JOIN training_rows AS j
  ON j.episode_id = r.episode_id
 AND j.stream_id = r.stream_id
 AND CAST(j.sample_index AS INT64) = CAST(r.sample_index AS INT64)
ORDER BY r.request_index, r.chunk_offset
"""

    def _fetch_actions(
        self, anchors: Sequence[Anchor]
    ) -> tuple[torch.Tensor, torch.Tensor, torch.Tensor]:
        batch_size = len(anchors)
        states: list[torch.Tensor | None] = [None] * batch_size
        actions: list[list[torch.Tensor | None]] = [
            [None] * self.chunk_size for _ in range(batch_size)
        ]
        padding = torch.ones((batch_size, self.chunk_size), dtype=torch.bool)
        required = ("request_index", "chunk_offset", "present", "state_value", "action_value")
        received = set()
        for row in self._torch_rows(self.db.sql(self._action_query(anchors)), required):
            request_index = int(row["request_index"])
            chunk_offset = int(row["chunk_offset"])
            coordinate = (request_index, chunk_offset)
            if (
                not 0 <= request_index < batch_size
                or not 0 <= chunk_offset < self.chunk_size
                or coordinate in received
            ):
                raise ValueError("Spiral returned an invalid or duplicate action coordinate")
            received.add(coordinate)
            present = bool(row["present"])
            if chunk_offset == 0:
                if not present:
                    raise ValueError("Spiral did not return the sampled anchor state")
                states[request_index] = _vector(row["state_value"], "state_value")
            if present:
                actions[request_index][chunk_offset] = _vector(
                    row["action_value"], "action_value"
                )
                padding[request_index, chunk_offset] = False
        if len(received) != batch_size * self.chunk_size or any(state is None for state in states):
            raise ValueError("Spiral returned an incomplete action manifest")
        action_tensor = torch.zeros((batch_size, self.chunk_size, self.action_dim))
        for request_index, row in enumerate(actions):
            for chunk_offset, value in enumerate(row):
                if value is not None:
                    if value.numel() != self.action_dim:
                        raise ValueError("the training query mixed action vector widths")
                    action_tensor[request_index, chunk_offset] = value
        state_tensor = torch.stack([value for value in states if value is not None])
        if state_tensor.shape[1] != self.state_dim:
            raise ValueError("the training query mixed state vector widths")
        return state_tensor, action_tensor, padding

    def _image_coordinates(self, anchors: Sequence[Anchor]) -> list[ImageCoordinate]:
        result = []
        for batch_index, anchor in enumerate(anchors):
            for camera_index, camera in enumerate(self.camera_streams):
                matches = []
                for segment in self.video_segments.get((anchor.episode_id, camera), ()):
                    frame_index = segment.frame_for(anchor.sample_index, anchor.sample_hz)
                    if frame_index is not None:
                        matches.append((segment, frame_index))
                if len(matches) != 1:
                    raise ValueError(
                        "the fixed-FPS contract did not resolve exactly one video frame for "
                        f"episode={anchor.episode_id!r}, sample={anchor.sample_index}, "
                        f"camera={camera!r}"
                    )
                segment, frame_index = matches[0]
                result.append(
                    ImageCoordinate(
                        batch_index,
                        camera_index,
                        anchor.episode_id,
                        camera,
                        segment.segment_index,
                        frame_index,
                        segment.media_relation,
                    )
                )
        return result

    @staticmethod
    def _frame_groups(coordinates: Sequence[ImageCoordinate]) -> list[FrameGroup]:
        grouped: dict[tuple[str, str, int, str], list[ImageCoordinate]] = defaultdict(list)
        for coordinate in coordinates:
            grouped[
                (
                    coordinate.episode_id,
                    coordinate.camera_stream_id,
                    coordinate.segment_index,
                    coordinate.media_relation,
                )
            ].append(coordinate)
        return [
            FrameGroup(
                group_id,
                key[0],
                key[1],
                key[2],
                key[3],
                tuple(item.frame_index for item in values),
                tuple((item.batch_index, item.camera_index) for item in values),
            )
            for group_id, (key, values) in enumerate(grouped.items())
        ]

    def _image_relation(self, group: FrameGroup) -> Any:
        item = pa.field("item", pa.int64(), nullable=False)
        list_type = pa.list_(item)
        schema = pa.schema(
            [
                pa.field("episode_id", pa.string(), nullable=False),
                pa.field("camera_stream_id", pa.string(), nullable=False),
                pa.field("segment_index", pa.uint32(), nullable=False),
                pa.field("requested_indices", list_type, nullable=False),
            ]
        )
        manifest = self.db.values(
            pa.table(
                {
                    "episode_id": [group.episode_id],
                    "camera_stream_id": [group.camera_stream_id],
                    "segment_index": pa.array([group.segment_index], type=pa.uint32()),
                    "requested_indices": pa.array(
                        [list(group.frame_indices)], type=list_type
                    ),
                },
                schema=schema,
            )
        )
        assets = self.db.table(group.media_relation).to_relation().select(
            {
                "asset_episode_id": sp.path("episode_id"),
                "asset_camera_stream_id": sp.path("stream_id"),
                "asset_segment_index": sp.path("segment_index"),
                # Attribute syntax collides with the relation's `.media`
                # namespace, so this column must always use an explicit path.
                "asset_media": sp.path("media"),
            }
        )
        sources = manifest.join_on(
            assets,
            [
                (sp.path("episode_id"), sp.path("asset_episode_id")),
                (sp.path("camera_stream_id"), sp.path("asset_camera_stream_id")),
                (sp.path("segment_index"), sp.path("asset_segment_index")),
            ],
        )
        selection = sp.ql.video.FrameIndices(sp._.requested_indices)
        frames = sources.video.frames(sp._.asset_media, selection)
        image = sp.ql.video.decode(sp._.frame, decoder=self.video_decoder)
        image = sp.ql.image.resize(image, self.image_width, self.image_height, "bilinear")
        image = sp.ql.image.to_tensor(image, "chw", "imagenet")
        return frames.select(
            {
                "request_index": sp._.request_index,
                "image": image,
            }
        )

    def _fetch_images(self, anchors: Sequence[Anchor]) -> tuple[torch.Tensor, int]:
        coordinates = self._image_coordinates(anchors)
        groups = self._frame_groups(coordinates)
        destinations = {
            (group.group_id, request_index): destination
            for group in groups
            for request_index, destination in enumerate(group.destinations)
        }
        images: list[list[torch.Tensor | None]] = [
            [None] * len(self.camera_streams) for _ in anchors
        ]
        transfer_bytes = 0
        seen = set()
        for group in groups:
            relation = self._image_relation(group)
            for row in self._torch_rows(relation, ("request_index", "image")):
                key = (group.group_id, int(row["request_index"]))
                destination = destinations.get(key)
                if destination is None or key in seen:
                    raise ValueError("Spiral returned an invalid or duplicate decoded-frame coordinate")
                seen.add(key)
                image = row["image"]
                if not isinstance(image, torch.Tensor):
                    image = torch.as_tensor(image)
                image = image.to(dtype=torch.float32)
                if image.shape != (3, self.image_height, self.image_width):
                    raise ValueError(
                        f"Spiral returned image shape {tuple(image.shape)}, expected "
                        f"(3, {self.image_height}, {self.image_width})"
                    )
                if not torch.isfinite(image).all():
                    raise ValueError("Spiral returned a non-finite image tensor")
                images[destination[0]][destination[1]] = image
                transfer_bytes += image.numel() * image.element_size()
        if len(seen) != len(destinations) or any(
            image is None for row in images for image in row
        ):
            raise ValueError("Spiral did not decode every requested image tensor")
        return (
            torch.stack(
                [torch.stack([image for image in row if image is not None]) for row in images]
            ),
            transfer_bytes,
        )

    def fetch(self, batch_size: int, validation: bool = False) -> ActBatch:
        started = time.perf_counter()
        if batch_size * self.chunk_size > MAX_MANIFEST_ROWS:
            raise ValueError(
                f"ACT action manifest has {batch_size * self.chunk_size:,} rows; "
                f"the supported maximum is {MAX_MANIFEST_ROWS:,}"
            )
        anchors = self._sample_anchors(validation, batch_size)
        state, actions, padding = self._fetch_actions(anchors)
        images, transfer_bytes = self._fetch_images(anchors)
        return ActBatch(
            state=state.to(self.device, non_blocking=True),
            images=images.to(self.device, non_blocking=True),
            actions=actions.to(self.device, non_blocking=True),
            action_is_pad=padding.to(self.device, non_blocking=True),
            metrics=LoaderMetrics(
                seconds=time.perf_counter() - started,
                source_bytes=transfer_bytes,
                requested_frames=len(anchors) * len(self.camera_streams),
            ),
        )


def self_test() -> None:
    assert _robot_type(None, "follower") == "so101_follower"
    assert _robot_type("so101_follower", "arm-7") == "so101_follower"
    media_path = (
        sp.Spiral()
        .values({"media": ["asset"]})
        .select({"asset_media": sp.path("media")})
        .collect()
        .to_arrow()
    )
    assert media_path.column("asset_media").to_pylist() == ["asset"]

    equal_rate_segment = VideoSegment(
        "episode_37", "wrist", 0, 0, 60, 100, "robotics.video_episode_37.assets"
    )
    assert equal_rate_segment.frame_for(42, Fraction(60, 1)) == 42

    segment = VideoSegment(
        "ep", "wrist", 0, 0, 30, 10, "robotics.video_ep.assets"
    )
    assert segment.sample_bounds(Fraction(60, 1)) == (0, 19)
    assert segment.frame_for(0, Fraction(60, 1)) == 0
    assert segment.frame_for(1, Fraction(60, 1)) == 0
    assert segment.frame_for(2, Fraction(60, 1)) == 1
    assert segment.frame_for(19, Fraction(60, 1)) == 9
    assert segment.frame_for(20, Fraction(60, 1)) is None
    assert SpiralActLoader._intersect_intervals(
        [(0, 9), (20, 29)], [(5, 24)]
    ) == [(5, 9), (20, 24)]
    coordinates = [
        ImageCoordinate(0, 0, "ep", "wrist", 0, 42, "robotics.video_ep.assets"),
        ImageCoordinate(1, 0, "ep", "wrist", 0, 7, "robotics.video_ep.assets"),
    ]
    groups = SpiralActLoader._frame_groups(coordinates)
    assert len(groups) == 1
    assert groups[0].frame_indices == (42, 7)
    assert groups[0].destinations == ((0, 0), (1, 0))
    assert groups[0].media_relation == "robotics.video_ep.assets"
    stats = NormalizationStats.normalized_joint_space(6, 6)
    assert torch.equal(stats.state_mean, torch.zeros(6))
    assert torch.equal(stats.action_std, torch.ones(6))


__all__ = [
    "ActBatch",
    "NormalizationStats",
    "SpiralActLoader",
    "TrainingCancelled",
    "self_test",
]
