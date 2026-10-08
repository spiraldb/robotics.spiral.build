"""Compare this port with SpiralDB's `spql.ai.warp_rm` on episodes of the hosted sample dataset.

Each episode's packets go through the service's own FastAPI app in-process, exactly as Studio sends
them; the Rust reference runs locally through Studio's release ADBC driver. Credentials are read at
runtime from the pyspiral login and `HF_TOKEN`, and never printed.

    uv run python parity.py [--device mps] [EPISODE_ID ...]
"""

import argparse
import json
import os
import time
import uuid
from pathlib import Path

import numpy as np
import pyarrow as pa
import torch
from adbc_driver_manager import dbapi
from fastapi.testclient import TestClient
from video_service.service import ARROW_STREAM

from warp_progress.artifacts import SELECTOR, Artifacts
from warp_progress.published import PublishedModels
from warp_progress.service import REVISION, ProgressService
from warp_progress.testing import LocalModelsVolume
from warp_progress.warp import WarpModel

DATASET = "dataset_warp_sim_bottles_sample"
CAMERA = "/cameras/top_camera-images-rgb"
# 408 frames (stride 13), 446 (stride 14), 473 (stride 15) and 1817 frames.
EPISODES = [
    "01a0b63c-9c55-72bd-ba55-853b05643eb4",
    "01a0b63c-b79b-77de-9411-647557eed17c",
    "01a0b63c-7c3b-7523-811a-9045921e085e",
    "01a0b63c-856c-778e-9702-d903a0159504",
]
ARTIFACTS = Path.home() / ".cache/warp-progress/artifacts"
# Parity scores with the released head alone, so no model is ever looked up here.
MODELS = Path.home() / ".cache/warp-progress/models"
STUDIO = Path.home() / "coding/spiral-robotics"
# Studio's packet shape (crates/common/src/datasets/camera_video.rs) for the episode bound to $1.
PACKETS = f"""packets AS (SELECT r.id, r.value.data AS payload,
  CAST(ROW_NUMBER() OVER (PARTITION BY r.id ORDER BY r.chunk_start_offset, r.sample_ordinal) - 1 AS BIGINT) AS decode_index,
  CAST(r.log_time AS BIGINT) - CAST(e.message_start_time AS BIGINT) AS pts_ns,
  spql.dt.from_epoch(CAST(r.log_time AS BIGINT), 'ns') - spql.dt.from_epoch(CAST(e.message_start_time AS BIGINT), 'ns') AS presentation_time
  FROM spql.mcap.read_topic('{DATASET}', '{CAMERA}') r
  JOIN spql.mcap.episodes('{DATASET}') e ON e.id = r.id WHERE r.id = $1)"""
REFERENCE = f"""WITH {PACKETS},
videos AS (SELECT id, spql.video.from_packets(spql.list.collect(STRUCT_PACK(payload := payload,
  decode_index := decode_index, presentation_time := presentation_time)), 'h264', 'annex_b') AS source
  FROM packets GROUP BY id)
SELECT spql.ai.warp_rm(source, model => '{SELECTOR}') AS scores FROM videos"""


class SpiralProject:
    """The user's hosted Spiral project, read through Studio's release ADBC driver."""

    def __init__(self):
        tokens = json.loads((Path.home() / "Library/Application Support/pyspiral/auth.json").read_text())
        self._connection = dbapi.connect(
            driver=str(STUDIO / "target/release/libadbc_driver_spiraldb.dylib"),
            entrypoint="AdbcDriverSpiraldbInit",
            db_kwargs={
                "uri": "spiraldb://?project=xiangpeng-robotics-618551",
                "spiral.api_url": "https://api.spiraldb.dev",
                "spiral.spfs_url": "https://spfs.spiraldb.dev",
                "spiral.local_cache": "true",
                "spiral.access_token": tokens["tokens"]["access_token"],
                "spiral.huggingface_token": os.environ["HF_TOKEN"],
            },
        )

    def query(self, sql: str, episode: str) -> pa.Table:
        parameters = pa.record_batch([pa.array([uuid.UUID(episode).bytes], pa.uuid())], names=["id"])
        with self._connection.cursor() as cursor:
            cursor.execute(sql, parameters)
            return cursor.fetch_arrow_table()

    def request(self, episode: str) -> bytes:
        """The episode's packets as the Arrow IPC body Studio posts."""
        packets = self.query(f"WITH {PACKETS} SELECT payload, decode_index, pts_ns FROM packets ORDER BY decode_index", episode)
        schema = pa.schema(
            [("payload", pa.binary()), ("decode_index", pa.int64()), ("pts_ns", pa.int64())],
            metadata={"codec": "h264", "framing": "annex_b", "model": SELECTOR, "implementation": REVISION},
        )
        table = packets.cast(schema)
        sink = pa.BufferOutputStream()
        with pa.ipc.new_stream(sink, schema) as writer:
            writer.write_table(table)
        return sink.getvalue().to_pybytes()

    def reference(self, episode: str) -> pa.Table:
        scores = self.query(REFERENCE, episode)["scores"].combine_chunks()
        return pa.Table.from_struct_array(scores.flatten())


def compare(ours: pa.Table, reference: pa.Table) -> dict[str, object]:
    def column(table: pa.Table, name: str) -> np.ndarray:
        return table[name].to_numpy(zero_copy_only=False)

    exact = {
        name: np.array_equal(column(ours, name), column(reference, name))
        for name in ("frame_index", "timestamp_ns", "window_stride", "support")
    }
    both_progress = ours["progress"].null_count == 0 and reference["progress"].null_count == 0
    return {
        "frames": (ours.num_rows, reference.num_rows),
        **exact,
        "velocity_max_abs": float(np.abs(column(ours, "velocity") - column(reference, "velocity")).max()),
        "progress_max_abs": float(np.abs(column(ours, "progress") - column(reference, "progress")).max())
        if both_progress
        else (ours["progress"].null_count, reference["progress"].null_count),
        "stride": int(column(ours, "window_stride")[0]),
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("episodes", nargs="*", default=EPISODES)
    parser.add_argument("--device", default="mps" if torch.backends.mps.is_available() else "cpu")
    arguments = parser.parse_args()
    started = time.perf_counter()
    published = PublishedModels(MODELS, LocalModelsVolume(MODELS))
    model = WarpModel(Artifacts(ARTIFACTS), published, torch.device(arguments.device))
    print(f"loaded on {arguments.device} in {time.perf_counter() - started:.1f} s")
    client = TestClient(ProgressService(model, "parity").app())
    project = SpiralProject()
    for episode in arguments.episodes:
        body = project.request(episode)
        started = time.perf_counter()
        response = client.post(
            "/score_progress",
            content=body,
            headers={"Authorization": "Bearer parity", "Content-Type": ARROW_STREAM},
        )
        seconds = time.perf_counter() - started
        response.raise_for_status()
        ours = pa.ipc.open_stream(response.content).read_all()
        print(episode, f"{seconds:.1f} s", compare(ours, project.reference(episode)))


if __name__ == "__main__":
    main()
