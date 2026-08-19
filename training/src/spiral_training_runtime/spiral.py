from __future__ import annotations

import math
import os
import re
import time
from dataclasses import dataclass
from itertools import pairwise
from typing import Any

import adbc_driver_manager
import numpy as np
import pyarrow as pa
from adbc_driver_manager import dbapi
from spiraldb.adbc import driver_path

from .arrow import TrainingArrays
from .errors import RuntimeRequestError

_IDENTIFIER = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
_MAX_ROWS = 2_000_000


def connect_spiral(options: dict[str, Any]) -> dbapi.Connection:
    """Open one database using only per-database ADBC options."""
    project = _project_id(options.get("project_id"))
    client_id = _optional_string(options.get("client_id"))
    client_secret = _optional_string(options.get("client_secret"))
    if (client_id is None) != (client_secret is None):
        raise RuntimeRequestError("Spiral client ID and secret must be provided together", 422)
    database_options: dict[str, Any] = {
        "driver": os.environ.get("SPIRALDB_ADBC_DRIVER") or driver_path(),
        "uri": f"spiraldb://?project={project}",
        "spiral.api_url": _service_url(options.get("api_url"), "API"),
        "spiral.spfs_url": _service_url(options.get("spfs_url"), "SpFS"),
    }
    if client_id is not None:
        database_options["username"] = client_id
        database_options["password"] = client_secret
    database = None
    native = None
    try:
        database = adbc_driver_manager.AdbcDatabase(**database_options)
        native = adbc_driver_manager.AdbcConnection(database)
        return dbapi.Connection(database, native, autocommit=True)
    except Exception:
        if native is not None:
            native.close()
        if database is not None:
            database.close()
        raise


@dataclass(frozen=True)
class Sample:
    episode_id: str
    stream_id: str
    sample_index: int
    episode_time_ns: int
    sample_hz: float
    state: np.ndarray
    action: np.ndarray
    robot_type: str
    joint_names: tuple[str, ...]


@dataclass(frozen=True)
class Anchor:
    sample: Sample
    frame_index: int
    episode_time_ns: int


@dataclass(frozen=True)
class VideoSegment:
    episode_id: str
    stream_id: str
    segment_index: int
    timeline_offset_ns: int
    fps: int
    frames: int
    width: int
    height: int
    media_relation: str

    def frame_at(self, episode_time_ns: int) -> int | None:
        relative = episode_time_ns - self.timeline_offset_ns
        if relative < 0:
            return None
        frame = math.floor(relative * self.fps / 1_000_000_000)
        return frame if 0 <= frame < self.frames else None


class SpiralActDataset:
    """Training-ready Arrow batches queried directly from SpiralDB."""

    def __init__(self, request: dict[str, Any]) -> None:
        self.request = request
        self.namespace = _identifier(request["spiral"].get("namespace"), "namespace")
        self.state_column = _identifier(request.get("state_column"), "state column")
        self.action_column = _identifier(request.get("action_column"), "action column")
        self.chunk_size = _positive_int(request.get("chunk_size"), "chunk size")
        self.batch_size = _positive_int(request.get("batch_size"), "batch size")
        self.random = np.random.default_rng(_nonnegative_int(request.get("seed"), "seed"))
        self.validation_random = np.random.default_rng(
            _nonnegative_int(request.get("seed"), "seed") ^ 0xA17C9E3D
        )
        self.connection = connect_spiral(request["spiral"])
        self.base_sql = _read_only_query(request.get("sql"))
        self.samples = self._load_samples()
        self.by_stream = self._index_samples(self.samples)
        self.joint_sample_hz = self.samples[0].sample_hz
        self.segments = self._load_segments()
        requested_cameras = tuple(request.get("act", {}).get("camera_streams") or ())
        self.camera_streams = requested_cameras or self._common_cameras()
        if not self.camera_streams:
            raise RuntimeRequestError(
                "the selected training episodes do not share a camera stream", 422
            )
        camera_rates = {
            segment.fps for segment in self.segments if segment.stream_id in self.camera_streams
        }
        if not camera_rates:
            raise RuntimeRequestError("the selected ACT cameras have no video segments", 422)
        if len(camera_rates) != 1:
            raise RuntimeRequestError("one ACT job requires one fixed camera frame rate", 422)
        self.sample_hz = float(next(iter(camera_rates)))
        if self.sample_hz > self.joint_sample_hz:
            raise RuntimeRequestError(
                "ACT camera frame rate cannot exceed the joint sample rate", 422
            )
        anchors = self._build_anchors()
        self.train_samples, self.validation_samples = self._split_samples(
            anchors, float(request.get("validation_split", 0.0))
        )
        self.state_mean, self.state_std = _normalization(
            [anchor.sample.state for anchor in self.train_samples]
        )
        self.action_mean, self.action_std = _normalization(
            [anchor.sample.action for anchor in self.train_samples]
        )
        self.state_dimensions = len(self.state_mean)
        self.action_dimensions = len(self.action_mean)
        self.robot_type = self.train_samples[0].sample.robot_type
        self.joint_names = self.train_samples[0].sample.joint_names
        requested_width = int(request.get("act", {}).get("image_width", 0))
        requested_height = int(request.get("act", {}).get("image_height", 0))
        if (requested_width == 0) != (requested_height == 0):
            raise RuntimeRequestError(
                "ACT image width and height must both be zero or positive", 422
            )
        dimensions = {
            (segment.width, segment.height)
            for segment in self.segments
            if segment.stream_id in self.camera_streams
        }
        if requested_width:
            self.image_width, self.image_height = requested_width, requested_height
            self.resize_images = True
        elif len(dimensions) == 1:
            self.image_width, self.image_height = next(iter(dimensions))
            self.resize_images = False
        else:
            raise RuntimeRequestError(
                "native-resolution ACT requires one camera size; set an explicit width and height",
                422,
            )
        self.video_decoder = str(request.get("act", {}).get("video_decoder", "auto"))

    @property
    def has_validation(self) -> bool:
        return bool(self.validation_samples)

    @property
    def summary(self) -> str:
        return (
            f"{len(self.train_samples)} train anchors, "
            f"{len(self.validation_samples)} validation anchors, "
            f"{len(self.camera_streams)} camera(s)"
        )

    def close(self) -> None:
        self.connection.close()

    def fetch(self, validation: bool = False) -> tuple[TrainingArrays, float]:
        started = time.perf_counter()
        population = self.validation_samples if validation else self.train_samples
        if not population:
            raise RuntimeRequestError("the requested episode split has no samples", 422)
        rng = self.validation_random if validation else self.random
        selected = self._select_episode_batch(population, rng)
        states = np.empty((self.batch_size, self.state_dimensions), dtype=np.float32)
        actions = np.zeros(
            (self.batch_size, self.chunk_size, self.action_dimensions), dtype=np.float32
        )
        padding = np.ones((self.batch_size, self.chunk_size), dtype=np.uint8)
        for row, anchor in enumerate(selected):
            states[row] = (anchor.sample.state - self.state_mean) / self.state_std
            stream = self.by_stream[(anchor.sample.episode_id, anchor.sample.stream_id)]
            for offset in range(self.chunk_size):
                action_index = math.floor(
                    (anchor.frame_index + offset) * self.joint_sample_hz / self.sample_hz
                )
                action_sample = stream.get(action_index)
                if action_sample is not None:
                    actions[row, offset] = (
                        action_sample.action - self.action_mean
                    ) / self.action_std
                    padding[row, offset] = 0
        images = self._fetch_images(selected)
        arrays = TrainingArrays(
            owner=None,
            states=np.ascontiguousarray(states),
            images=np.ascontiguousarray(images.reshape(self.batch_size, -1)),
            actions=np.ascontiguousarray(actions.reshape(self.batch_size, -1)),
            action_is_pad=np.ascontiguousarray(padding),
        )
        return arrays, (time.perf_counter() - started) * 1_000.0

    def _select_episode_batch(
        self, population: list[Anchor], rng: np.random.Generator
    ) -> list[Anchor]:
        # Selecting the episode through a uniformly sampled anchor weights each
        # episode by its population. Sampling within that episode therefore
        # preserves the uniform marginal probability of every training anchor,
        # while keeping one batch local to a small set of video segments.
        episode_id = population[int(rng.integers(0, len(population)))].sample.episode_id
        episode = [anchor for anchor in population if anchor.sample.episode_id == episode_id]
        return [episode[index] for index in rng.integers(0, len(episode), self.batch_size)]

    def write_checkpoint(
        self,
        run_id: str,
        run_name: str,
        step: int,
        created_ms: int,
        validation_loss: float | None,
        config_json: str,
        payload: bytes,
    ) -> None:
        table = pa.table(
            {
                "run_id": [run_id],
                "run_name": [run_name],
                "policy": ["act"],
                "step": pa.array([step], type=pa.uint64()),
                "created_ms": pa.array([created_ms], type=pa.int64()),
                "validation_loss": pa.array([validation_loss], type=pa.float64()),
                "config_json": [config_json],
                "checkpoint": pa.array([payload], type=pa.large_binary()),
            }
        )
        with self.connection.cursor() as cursor:
            cursor.adbc_ingest(
                f"{self.namespace}.training_checkpoints", table, mode="create_append"
            )

    def _query(self, sql: str) -> pa.Table:
        try:
            with self.connection.cursor() as cursor:
                cursor.execute(sql)
                return cursor.fetch_arrow_table()
        except Exception as error:
            raise RuntimeRequestError(f"Spiral training query failed: {error}", 422) from error

    def _load_samples(self) -> list[Sample]:
        table = self._query(
            f"WITH training_rows AS ({self.base_sql}) "
            "SELECT episode_id, stream_id, sample_index, episode_time_ns, sample_hz, "
            f"robot_type, joint_names, {self.state_column} AS state_value, "
            f"{self.action_column} AS action_value FROM training_rows "
            "ORDER BY episode_id, stream_id, sample_index"
        )
        if table.num_rows == 0:
            raise RuntimeRequestError("the Spiral training query returned no rows", 422)
        if table.num_rows > _MAX_ROWS:
            raise RuntimeRequestError(
                f"the training selection exceeds the {_MAX_ROWS}-row worker limit", 422
            )
        columns = {name: table[name].to_pylist() for name in table.column_names}
        result = []
        state_width = action_width = None
        for row in range(table.num_rows):
            state = _vector(columns["state_value"][row], "state")
            action = _vector(columns["action_value"][row], "action")
            state_width = state_width or len(state)
            action_width = action_width or len(action)
            if len(state) != state_width or len(action) != action_width:
                raise RuntimeRequestError("training rows mixed vector widths", 422)
            sample_hz = float(columns["sample_hz"][row])
            if not math.isfinite(sample_hz) or sample_hz <= 0:
                raise RuntimeRequestError("training rows contain an invalid sample rate", 422)
            result.append(
                Sample(
                    episode_id=str(columns["episode_id"][row]),
                    stream_id=str(columns["stream_id"][row]),
                    sample_index=int(columns["sample_index"][row]),
                    episode_time_ns=int(columns["episode_time_ns"][row]),
                    sample_hz=sample_hz,
                    state=state,
                    action=action,
                    robot_type=str(columns["robot_type"][row] or "so101_follower"),
                    joint_names=tuple(str(value) for value in columns["joint_names"][row]),
                )
            )
        if len({sample.sample_hz for sample in result}) != 1:
            raise RuntimeRequestError("one ACT job requires one fixed sample rate", 422)
        if len({(sample.robot_type, sample.joint_names) for sample in result}) != 1:
            raise RuntimeRequestError("training rows mixed robot or joint contracts", 422)
        return result

    @staticmethod
    def _index_samples(samples: list[Sample]) -> dict[tuple[str, str], dict[int, Sample]]:
        streams: dict[tuple[str, str], dict[int, Sample]] = {}
        for sample in samples:
            values = streams.setdefault((sample.episode_id, sample.stream_id), {})
            if sample.sample_index in values:
                raise RuntimeRequestError("training rows contain duplicate sample indices", 422)
            values[sample.sample_index] = sample
        for key, values in streams.items():
            indices = sorted(values)
            if indices != list(range(indices[0], indices[-1] + 1)):
                raise RuntimeRequestError(f"training stream {key!r} is not contiguous", 422)
        return streams

    def _split_samples(
        self, samples: list[Anchor], validation_split: float
    ) -> tuple[list[Anchor], list[Anchor]]:
        if not math.isfinite(validation_split) or not 0.0 <= validation_split <= 0.5:
            raise RuntimeRequestError("validation split must be between 0 and 0.5", 422)
        episodes = sorted({sample.sample.episode_id for sample in samples})
        if validation_split == 0.0 or len(episodes) < 2:
            return samples, []
        seed = int(self.request["seed"])
        episodes.sort(key=lambda episode: (_episode_score(seed, episode), episode))
        validation_count = min(len(episodes) - 1, max(1, round(len(episodes) * validation_split)))
        validation_ids = set(episodes[:validation_count])
        return (
            [sample for sample in samples if sample.sample.episode_id not in validation_ids],
            [sample for sample in samples if sample.sample.episode_id in validation_ids],
        )

    def _load_segments(self) -> list[VideoSegment]:
        table = self._query(
            "SELECT episode_id, stream_id, segment_index, timeline_offset_ns, fps, frames, "
            f"width, height, media_relation FROM {self.namespace}.video_assets"
        )
        episodes = {sample.episode_id for sample in self.samples}
        result = []
        for raw in table.to_pylist():
            if raw["episode_id"] not in episodes:
                continue
            relation = str(raw["media_relation"])
            if not all(_IDENTIFIER.fullmatch(part) for part in relation.split(".")):
                raise RuntimeRequestError(f"invalid media relation {relation!r}", 422)
            if (
                int(raw["fps"]) <= 0
                or int(raw["frames"]) <= 0
                or int(raw["width"]) <= 0
                or int(raw["height"]) <= 0
            ):
                raise RuntimeRequestError("camera segment has an invalid video contract", 422)
            result.append(
                VideoSegment(
                    episode_id=str(raw["episode_id"]),
                    stream_id=str(raw["stream_id"]),
                    segment_index=int(raw["segment_index"]),
                    timeline_offset_ns=int(raw["timeline_offset_ns"] or 0),
                    fps=int(raw["fps"]),
                    frames=int(raw["frames"]),
                    width=int(raw["width"]),
                    height=int(raw["height"]),
                    media_relation=relation,
                )
            )
        grouped: dict[tuple[str, str], list[VideoSegment]] = {}
        for segment in result:
            grouped.setdefault((segment.episode_id, segment.stream_id), []).append(segment)
        for key, segments in grouped.items():
            segments.sort(key=lambda segment: (segment.timeline_offset_ns, segment.segment_index))
            for current, following in pairwise(segments):
                current_end = current.timeline_offset_ns + math.ceil(
                    current.frames * 1_000_000_000 / current.fps
                )
                if following.timeline_offset_ns < current_end:
                    raise RuntimeRequestError(
                        f"camera stream {key!r} has overlapping video segments", 422
                    )
        return result

    def _common_cameras(self) -> tuple[str, ...]:
        episodes = {sample.episode_id for sample in self.samples}
        cameras_by_episode = {
            episode: {
                segment.stream_id for segment in self.segments if segment.episode_id == episode
            }
            for episode in episodes
        }
        common = set.intersection(*cameras_by_episode.values()) if cameras_by_episode else set()
        return tuple(sorted(common))

    def _build_anchors(self) -> list[Anchor]:
        anchors: list[Anchor] = []
        seen: set[tuple[str, str, int]] = set()
        reference_camera = self.camera_streams[0]
        for (episode_id, stream_id), samples in self.by_stream.items():
            reference_segments = [
                segment
                for segment in self.segments
                if segment.episode_id == episode_id and segment.stream_id == reference_camera
            ]
            for segment in reference_segments:
                first_frame = math.floor(
                    segment.timeline_offset_ns * self.sample_hz / 1_000_000_000
                )
                last_frame = (
                    math.ceil(segment.timeline_offset_ns * self.sample_hz / 1_000_000_000)
                    + segment.frames
                )
                for frame_index in range(first_frame, last_frame + 1):
                    if frame_index < 0:
                        continue
                    episode_time_ns = round(frame_index * 1_000_000_000 / self.sample_hz)
                    if segment.frame_at(episode_time_ns) is None:
                        continue
                    key = (episode_id, stream_id, frame_index)
                    if key in seen:
                        raise RuntimeRequestError(
                            f"camera {reference_camera!r} has overlapping video segments", 422
                        )
                    if not all(
                        sum(
                            candidate.episode_id == episode_id
                            and candidate.stream_id == camera
                            and candidate.frame_at(episode_time_ns) is not None
                            for candidate in self.segments
                        )
                        == 1
                        for camera in self.camera_streams
                    ):
                        continue
                    joint_index = math.floor(frame_index * self.joint_sample_hz / self.sample_hz)
                    sample = samples.get(joint_index)
                    if sample is None:
                        continue
                    seen.add(key)
                    anchors.append(Anchor(sample, frame_index, episode_time_ns))
        if not anchors:
            raise RuntimeRequestError(
                "the training query has no samples with complete synchronized camera coverage",
                422,
            )
        return anchors

    def _fetch_images(self, samples: list[Anchor]) -> np.ndarray:
        output = np.empty(
            (
                len(samples),
                len(self.camera_streams),
                self.image_height,
                self.image_width,
                3,
            ),
            dtype=np.uint8,
        )
        groups: dict[VideoSegment, list[tuple[int, int, int]]] = {}
        for batch_index, anchor in enumerate(samples):
            for camera_index, camera in enumerate(self.camera_streams):
                segment = next(
                    segment
                    for segment in self.segments
                    if segment.episode_id == anchor.sample.episode_id
                    and segment.stream_id == camera
                    and segment.frame_at(anchor.episode_time_ns) is not None
                )
                frame = segment.frame_at(anchor.episode_time_ns)
                assert frame is not None
                groups.setdefault(segment, []).append((batch_index, camera_index, frame))
        grouped = list(groups.items())
        decoded = f"spql.video.decode(f.frame, {_literal(self.video_decoder)})"
        if self.resize_images:
            decoded = (
                f"spql.image.resize({decoded}, {self.image_width}, {self.image_height}, 'bilinear')"
            )
        expression = f"spql.image.to_tensor({decoded}, 'hwc', 'none')"
        selects = []
        for group_index, (segment, destinations) in enumerate(grouped):
            indices = ", ".join(str(destination[2]) for destination in destinations)
            selects.append(
                f"SELECT {group_index} AS group_index, "
                "CAST(f.request_index AS BIGINT) AS request_index, "
                f"{expression} AS image FROM "
                f"(SELECT media FROM {segment.media_relation} WHERE "
                f"episode_id = {_literal(segment.episode_id)} AND "
                f"stream_id = {_literal(segment.stream_id)} AND "
                f"segment_index = {segment.segment_index}) AS a CROSS JOIN LATERAL "
                "spql.video.frames(a.media, "
                f"spql.video.frame_indices(ARRAY[{indices}])) AS f"
            )
        table = self._query(" UNION ALL ".join(selects) + " ORDER BY group_index, request_index")
        rows = table.to_pylist()
        expected_rows = sum(len(destinations) for _, destinations in grouped)
        if len(rows) != expected_rows:
            raise RuntimeRequestError("Spiral did not decode every requested image", 422)
        for row in rows:
            raw_group_index = row["group_index"]
            raw_request_index = row["request_index"]
            try:
                group_index = int(raw_group_index)
                request_index = int(raw_request_index)
                destinations = grouped[group_index][1]
                batch_index, camera_index, _ = destinations[request_index]
            except (IndexError, TypeError, ValueError) as error:
                raise RuntimeRequestError(
                    "Spiral returned invalid video request coordinates: "
                    f"group {raw_group_index!r}, request {raw_request_index!r}",
                    422,
                ) from error
            image = _image_tensor(row["image"], self.image_height, self.image_width)
            output[batch_index, camera_index] = image
        return output


def _read_only_query(value: Any) -> str:
    if not isinstance(value, str) or not value.strip():
        raise RuntimeRequestError("training SQL must be non-empty", 422)
    query = value.strip().removesuffix(";").strip()
    if ";" in query or not query.lower().startswith(("select", "with")):
        raise RuntimeRequestError("training SQL must be one read-only SELECT query", 422)
    return query


def _identifier(value: Any, label: str) -> str:
    if not isinstance(value, str) or _IDENTIFIER.fullmatch(value) is None:
        raise RuntimeRequestError(f"invalid Spiral {label}", 422)
    return value


def _project_id(value: Any) -> str:
    if not isinstance(value, str) or re.fullmatch(r"[A-Za-z0-9_-]+", value) is None:
        raise RuntimeRequestError("invalid Spiral project ID", 422)
    return value


def _service_url(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value.startswith(("http://", "https://")):
        raise RuntimeRequestError(f"Spiral {label} endpoint must be HTTP or HTTPS", 422)
    return value.rstrip("/")


def _optional_string(value: Any) -> str | None:
    return value.strip() if isinstance(value, str) and value.strip() else None


def _image_tensor(value: Any, height: int, width: int) -> np.ndarray:
    expected_shape = [height, width, 3]
    if isinstance(value, dict):
        shape = value.get("shape")
        if shape != expected_shape:
            raise RuntimeRequestError(
                f"Spiral returned image tensor shape {shape!r}, expected {expected_shape!r}",
                422,
            )
        value = value.get("data")
    image = np.asarray(value, dtype=np.uint8)
    expected_size = height * width * 3
    if image.size != expected_size:
        raise RuntimeRequestError(
            f"Spiral returned {image.size} image tensor values, expected {expected_size}",
            422,
        )
    return image.reshape(height, width, 3)


def _positive_int(value: Any, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise RuntimeRequestError(f"{label} must be a positive integer", 422)
    return value


def _nonnegative_int(value: Any, label: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise RuntimeRequestError(f"{label} must be a nonnegative integer", 422)
    return value


def _vector(value: Any, label: str) -> np.ndarray:
    result = np.asarray(value, dtype=np.float32)
    if result.ndim != 1 or result.size == 0 or not np.isfinite(result).all():
        raise RuntimeRequestError(f"training {label} must be one finite numeric vector", 422)
    return result


def _normalization(values: list[np.ndarray]) -> tuple[np.ndarray, np.ndarray]:
    matrix = np.stack(values).astype(np.float64)
    mean = matrix.mean(axis=0)
    std = matrix.std(axis=0)
    std = np.where(std < 1.0e-8, 1.0, std)
    return mean.astype(np.float32), std.astype(np.float32)


def _episode_score(seed: int, episode_id: str) -> int:
    value = 0xCBF29CE484222325 ^ seed
    for byte in episode_id.encode():
        value ^= byte
        value = (value * 0x00000100000001B3) & 0xFFFFFFFFFFFFFFFF
    value ^= value >> 30
    value = (value * 0xBF58476D1CE4E5B9) & 0xFFFFFFFFFFFFFFFF
    value ^= value >> 27
    value = (value * 0x94D049BB133111EB) & 0xFFFFFFFFFFFFFFFF
    return value ^ (value >> 31)


def _literal(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"
