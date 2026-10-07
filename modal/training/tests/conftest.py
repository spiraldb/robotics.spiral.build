from __future__ import annotations

import json
from fractions import Fraction
from pathlib import Path

import av
import numpy as np
import pyarrow as pa
import pytest
from fastapi.testclient import TestClient

from spiral_training_runtime.control import ControlService, RunControl
from spiral_training_runtime.request import RunRequest
from spiral_training_runtime.volume import TrainingVolume

FIXTURES = (
    Path(__file__).resolve().parents[3] / "crates" / "backend" / "src" / "training" / "fixtures"
)
TOKEN = "secret"
AUTHORIZED = {"Authorization": f"Bearer {TOKEN}"}
CAMERA_RATE = 15
VALUE_RATE = 50
NANOSECONDS = 1_000_000_000


def fixture(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text(encoding="utf-8"))


def arrow_stream(table: pa.Table) -> bytes:
    sink = pa.BufferOutputStream()
    with pa.ipc.new_stream(sink, table.schema) as writer:
        for batch in table.to_batches(max_chunksize=7):
            writer.write_batch(batch)
    return sink.getvalue().to_pybytes()


class SyntheticCamera:
    """A small libx264 Annex-B stream with B-frames, so decode order differs from presentation
    order; picture `i` is one flat grey, `shade(i)`."""

    @staticmethod
    def shade(index: int) -> int:
        return 40 + (index * 23) % 180

    @classmethod
    def table(cls, start_ns: int, pictures: int, size: tuple[int, int] = (96, 72)) -> pa.Table:
        width, height = size
        encoder = av.CodecContext.create("libx264", "w")
        encoder.width, encoder.height, encoder.pix_fmt = width, height, "yuv420p"
        encoder.time_base = Fraction(1, CAMERA_RATE)
        packets = []
        for index in range(pictures):
            rgb = np.full((height, width, 3), cls.shade(index), np.uint8)
            frame = av.VideoFrame.from_ndarray(rgb, format="rgb24").reformat(format="yuv420p")
            frame.pts = index
            packets += encoder.encode(frame)
        packets += encoder.encode(None)
        times = [start_ns + packet.pts * NANOSECONDS // CAMERA_RATE for packet in packets]
        return pa.table(
            {
                "log_time_ns": pa.array(times, pa.int64()),
                "payload": pa.array([bytes(packet) for packet in packets], pa.binary()),
            }
        ).replace_schema_metadata({"codec": "h264", "framing": "annex_b"})


def value_table(times_ns: np.ndarray, values: np.ndarray) -> pa.Table:
    columns = {"log_time_ns": pa.array(times_ns, pa.int64())}
    for index in range(values.shape[1]):
        columns[f"v{index}"] = pa.array(values[:, index], pa.float64())
    return pa.table(columns)


class LocalSync:
    """A Volume mount that is the only one: nothing to commit or reload."""

    def commit(self) -> None:
        pass

    def reload(self) -> None:
        pass


class FakeCalls:
    """Function calls that never run on their own: a test runs or ends them."""

    def __init__(self) -> None:
        self.spawned: list[str] = []
        self.cancelled: list[str] = []
        self.finished: set[str] = set()

    def spawn(self, request_json: str) -> str:
        self.spawned.append(request_json)
        return f"call-{len(self.spawned)}"

    def ended(self, call_id: str) -> bool:
        return call_id in self.finished

    def cancel(self, call_id: str) -> None:
        self.cancelled.append(call_id)
        self.finished.add(call_id)


class SyntheticRun:
    """A run request's streams, synthesized on a volume: 15 fps cameras and 50 Hz joint values
    covering each episode's intervals with a margin on both sides."""

    def __init__(self, volume: TrainingVolume, request: RunRequest) -> None:
        self.volume = volume
        self.request = request

    @classmethod
    def from_fixture(cls, volume: TrainingVolume) -> SyntheticRun:
        return cls(volume, RunRequest.model_validate(fixture("run-request.json")))

    def write(self) -> None:
        for episode in self.request.episodes:
            start = min(start for start, _ in episode.intervals) - NANOSECONDS // 10
            end = max(end for _, end in episode.intervals) + NANOSECONDS // 5
            pictures = (end - start) * CAMERA_RATE // NANOSECONDS
            for key in episode.cameras:
                self.store(key, SyntheticCamera.table(start, pictures))
            times = np.arange(start, end, NANOSECONDS // VALUE_RATE)
            for keys, joints, phase in (
                (episode.state, self.request.joints.state, 0.0),
                (episode.action, self.request.joints.action, 0.3),
            ):
                values = np.sin(
                    np.outer(times / NANOSECONDS + phase, np.arange(1, len(joints) + 1))
                )
                widths = [len(joints) - len(keys) + 1] + [1] * (len(keys) - 1)
                for key, columns in zip(
                    keys, np.split(values, np.cumsum(widths)[:-1], axis=1), strict=True
                ):
                    self.store(key, value_table(times, columns))

    def store(self, key: str, table: pa.Table) -> None:
        path = self.volume.stream(key)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(arrow_stream(table))


@pytest.fixture
def volume(tmp_path: Path) -> TrainingVolume:
    return TrainingVolume(tmp_path / "volume", LocalSync())


@pytest.fixture
def calls() -> FakeCalls:
    return FakeCalls()


@pytest.fixture
def stop_flags() -> dict[str, bool]:
    return {}


@pytest.fixture
def client(volume: TrainingVolume, calls: FakeCalls, stop_flags: dict[str, bool]) -> TestClient:
    runs = RunControl(volume, calls, stop_flags)
    return TestClient(ControlService(volume, runs, TOKEN).app())
