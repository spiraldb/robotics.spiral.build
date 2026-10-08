"""WARP-RM sim-bottles task progress over one episode's 30 Hz top-camera video.

A PyTorch port of SpiralDB's `spql.ai.warp_rm`: engine/plugins/spiral-ai/src/warp.rs,
warp/dinov3.rs and warp/temporal.rs. The names below follow theirs.
"""

import itertools
import threading
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path

import numpy as np
import pyarrow as pa
import torch
from torch import nn
from transformers import DINOv3ViTModel

from video_service.episode import EpisodeVideo, RejectedVideo
from video_service.timing import StageTimes

from warp_progress.artifacts import SELECTOR, Artifacts
from warp_progress.published import PublishedModels

# Pictures the backbone embeds per batch: fp16 on a T4 takes 5.84 s for 1,817 pictures at 16,
# 5.58 s at 64 and 5.49 s at 128, which doubles each request's host buffer for 2% more.
BATCH_FRAMES = 64
# The shortest episode the temporal windows can score.
MIN_FRAMES = 33
# The checkpoint's 30 Hz frame period, and how far a picture's may stray from it.
FRAME_PERIOD_NS = 1_000_000_000 // 30
FRAME_PERIOD_TOLERANCE_NS = 1_000_000
IMAGENET_MEAN = np.array([0.485, 0.456, 0.406], np.float32)
IMAGENET_STD = np.array([0.229, 0.224, 0.225], np.float32)
# One row per picture, as the elements of the Rust function's list.
OUTPUT_SCHEMA = pa.schema(
    [
        pa.field("frame_index", pa.uint32(), nullable=False),
        pa.field("timestamp_ns", pa.uint64(), nullable=False),
        pa.field("velocity", pa.float32(), nullable=False),
        pa.field("progress", pa.float32()),
        pa.field("window_stride", pa.uint32(), nullable=False),
        pa.field("support", pa.uint32(), nullable=False),
    ]
)


class WarpModel:
    """The official DINOv3 ViT-B/16 backbone and WARP-RM temporal heads on one device: the released
    sim-bottles head, and each published progress model, loaded the first time a request names it
    and kept.

    Concurrent requests decode on their own threads and take turns on the device, one batch at a
    time, so the device serves one request's work at a time and never holds two batches.
    """

    def __init__(self, artifacts: Artifacts, published: PublishedModels, device: torch.device):
        self._encoder = PictureEncoder(artifacts.backbone, device)
        self._published = published
        self._device = device
        # The heads loaded so far, by the `model` requests name them by.
        self._heads = {SELECTOR: TemporalModel.load(artifacts.head).to(device)}
        # Heads load one at a time, so each loads once and the models Volume reloads with none of
        # its files open.
        self._loading = threading.Lock()

    def score(self, video: EpisodeVideo, model: str | None, times: StageTimes) -> pa.RecordBatch:
        """Every picture's velocity and progress by the head `model` names, in display order. A
        model it cannot score with is refused before the video is decoded."""
        temporal = self._head(model)
        with torch.inference_mode():
            timestamps, embeddings = [], []
            for batch_times, batch_embeddings in self._encoder.encode(video, times):
                timestamps.extend(batch_times)
                embeddings.append(batch_embeddings)
            clock = PresentationTimes(timestamps)
            encoder = self._encoder
            with encoder.device_turn(times), encoder.device_stage("temporal", times), encoder.precision():
                scores = temporal.score(torch.cat(embeddings))
        return scores.record_batch(clock.nanoseconds())

    def _head(self, model: str | None) -> "TemporalModel":
        """The released head, or a published one, loaded as the released head is."""
        with self._loading:
            if model not in self._heads:
                tensors = self._published.tensors(model)
                self._heads[model] = TemporalModel.from_tensors(tensors).to(self._device)
            return self._heads[model]


class PictureEncoder:
    """The official DINOv3 ViT-B/16 backbone on one device: each picture of a video as its final-norm
    CLS embedding, as the scorer reads it. The training job embeds the pictures it trains on with it.

    Callers take turns on the device, one batch at a time, so the device never holds two batches.
    """

    def __init__(self, backbone: Path, device: torch.device):
        """The backbone in `backbone`, the directory holding its `config.json` and `model.safetensors`."""
        self._device = device
        self._backbone = DINOv3ViTModel.from_pretrained(backbone).to(device).eval()
        self._device_lock = threading.Lock()
        # Squash tables per picture size, touched only while holding the device.
        self._squashes: dict[tuple[int, int], SquashPreprocessing] = {}

    @torch.inference_mode()
    def encode(self, video: EpisodeVideo, times: StageTimes) -> Iterator[tuple[list[int], torch.Tensor]]:
        """Every picture in display order, a batch at a time: the batch's presentation times and its
        `(pictures, 768)` embeddings."""
        pictures = times.timed("decode", self._pictures(video))
        for _, same_size in itertools.groupby(pictures, key=lambda picture: picture[1].shape):
            for batch in itertools.batched(same_size, BATCH_FRAMES):
                yield [time_ns for time_ns, _ in batch], self._embed([rgb for _, rgb in batch], times)

    def precision(self) -> torch.autocast:
        """fp16 autocast on CUDA, about 3x faster on a T4; full precision elsewhere, the closest
        match to the Rust for local parity runs."""
        return torch.autocast(self._device.type, dtype=torch.float16, enabled=self._device.type == "cuda")

    @contextmanager
    def device_turn(self, times: StageTimes) -> Iterator[None]:
        """Exclusive use of the device; the wait for it is the `queue` stage."""
        with times.stage("queue"):
            self._device_lock.acquire()
        try:
            yield
        finally:
            self._device_lock.release()

    @contextmanager
    def device_stage(self, name: str, times: StageTimes) -> Iterator[None]:
        """A stage of device work, timed until the device has finished it."""
        with times.stage(name):
            yield
            if self._device.type != "cpu":
                torch.accelerator.synchronize(self._device)

    @staticmethod
    def _pictures(video: EpisodeVideo) -> Iterator[tuple[int, np.ndarray]]:
        """Every picture in display order, with its presentation time and its height x width x 3 RGB24
        pixels converted as spiral-video converts them: swscale at the decoded size with
        FAST_BILINEAR, using the matrix and range the stream signals, so an untagged stream converts as
        BT.601 limited range."""
        for frame in video.frames():
            yield frame.pts, frame.to_ndarray(format="rgb24", interpolation="FAST_BILINEAR")

    def _embed(self, pictures: list[np.ndarray], times: StageTimes) -> torch.Tensor:
        """The backbone's final-norm CLS embedding of each picture, all of one size."""
        with times.stage("copy"):
            rgb = torch.from_numpy(np.stack(pictures))
        with self.device_turn(times):
            with self.device_stage("copy", times):
                rgb = rgb.to(self._device)
            with self.device_stage("squash", times):
                size = rgb.shape[1:3]
                if size not in self._squashes:
                    self._squashes[size] = SquashPreprocessing(size, self._device)
                inputs = self._squashes[size].apply(rgb)
            with self.device_stage("backbone", times), self.precision():
                return self._backbone(pixel_values=inputs).pooler_output


class PresentationTimes:
    """One episode's picture times, checked against the checkpoint's 30 Hz clock."""

    def __init__(self, times_ns: list[int]):
        if len(times_ns) < MIN_FRAMES:
            raise RejectedVideo(
                f"WARP requires an episode of at least {MIN_FRAMES} pictures, got {len(times_ns)}"
            )
        self.require_30_hz(times_ns)
        if times_ns[0] < 0:
            raise RejectedVideo(f"WARP picture time {times_ns[0]} ns precedes its episode")
        self._times_ns = times_ns

    @staticmethod
    def require_30_hz(times_ns: list[int]) -> None:
        """Rejects pictures in display order that stray from the checkpoint's 30 Hz clock."""
        for index, gap in enumerate(np.diff(times_ns), start=1):
            if abs(gap - FRAME_PERIOD_NS) > FRAME_PERIOD_TOLERANCE_NS:
                raise RejectedVideo(
                    "Pinned WARP sim-bottles model requires a 30 Hz video; "
                    f"picture {index} follows the previous one by {gap} ns"
                )

    def nanoseconds(self) -> np.ndarray:
        return np.array(self._times_ns, np.uint64)


class SquashPreprocessing:
    """RGB resized straight to 224x224 with OpenCV's half-pixel bilinear geometry and 11-bit
    interpolation coefficients, then ImageNet-normalized, on the model's device.

    The geometry and the normalization are tabulated on the CPU in float64 and float32, as the Rust
    computes them; the device only gathers and sums integers, so every device yields the same bits.
    """

    def __init__(self, size: tuple[int, int], device: torch.device):
        """Tables for `(height, width)` pictures."""
        (y0, y1, b0, b1), (x0, x1, a0, a1) = (self._axis(source) for source in size)
        # Output pixel p blends source pixels taps[:, p] with weights out of 2048 * 2048, which
        # sum below 2**31 even for 255 in every channel.
        rows, row_weights = np.stack([y0, y0, y1, y1]), np.stack([b0, b0, b1, b1])
        columns, column_weights = np.stack([x0, x1, x0, x1]), np.stack([a0, a1, a0, a1])
        taps = rows[:, :, None] * size[1] + columns[:, None, :]
        weights = row_weights[:, :, None] * column_weights[:, None, :]
        self._taps = torch.from_numpy(taps.reshape(4, -1)).to(device)
        self._weights = torch.from_numpy(weights.reshape(4, -1, 1).astype(np.int32)).to(device)
        # Every channel level as `(level / 255 - mean) / std`.
        levels = np.arange(256, dtype=np.float32)[:, None]
        normalized = (levels / np.float32(255.0) - IMAGENET_MEAN) / IMAGENET_STD
        self._normalized = torch.from_numpy(normalized).to(device)

    @staticmethod
    def _axis(source: int) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
        """Per output position: the two source indices it blends and their weights out of 2048."""
        position = (np.arange(224) + 0.5) * source / 224.0 - 0.5
        origin = np.floor(position).astype(np.int64)
        fraction = np.where((origin < 0) | (origin >= source - 1), 0.0, position - origin)
        first = origin.clip(0, source - 1)
        return (
            first,
            np.minimum(first + 1, source - 1),
            np.rint((1.0 - fraction) * 2048.0).astype(np.int64),
            np.rint(fraction * 2048.0).astype(np.int64),
        )

    def apply(self, rgb: torch.Tensor) -> torch.Tensor:
        """`(pictures, height, width, 3)` uint8 on the device as the backbone's `(pictures, 3, 224, 224)`."""
        taps = rgb.reshape(len(rgb), -1, 3)[:, self._taps].int()
        blended = (taps * self._weights).sum(dim=1, dtype=torch.int32)
        levels = ((blended + (1 << 21)) >> 22).clamp(0, 255).long()
        normalized = self._normalized.gather(0, levels.reshape(-1, 3))
        return normalized.reshape(len(rgb), 224, 224, 3).permute(0, 3, 1, 2).contiguous()


class TemporalModel(nn.Module):
    """Released WARP-RM's bidirectional, post-norm Transformer over 32 steps of CLS embeddings."""

    def __init__(self):
        super().__init__()
        self.input_proj = nn.Linear(1536, 768, bias=False)
        self.pos_embed = nn.Parameter(torch.empty(1, 32, 768))
        layer = nn.TransformerEncoderLayer(768, 8, 3072, dropout=0.0, batch_first=True)
        self.transformer = nn.TransformerEncoder(layer, 12)
        self.reward_head = nn.Linear(768, 30)
        self.register_buffer("rel_bin_centers", torch.empty(30))

    @classmethod
    def load(cls, checkpoint: Path) -> "TemporalModel":
        """The released head, from its PyTorch checkpoint."""
        return cls.from_tensors(torch.load(checkpoint, map_location="cpu", weights_only=True)["model"])

    @classmethod
    def from_tensors(cls, tensors: dict[str, torch.Tensor]) -> "TemporalModel":
        """A head from its tensors under upstream `TransformerAggregator` names, as the released head
        and the published progress models hold them."""
        model = cls()
        # Checkpoints also carry heads the scorer never reads.
        model.load_state_dict({name: tensors[name] for name in model.state_dict()})
        return model.eval()

    def forward(self, features: torch.Tensor) -> torch.Tensor:
        """Each window's relative progress at its 32 steps, from its `(windows, 32, 768)` embeddings."""
        differences = torch.cat([torch.zeros_like(features[:, :1]), features.diff(dim=1)], dim=1)
        hidden = self.input_proj(torch.cat([features, differences], dim=2)) + self.pos_embed
        bins = self.reward_head(self.transformer(hidden)).softmax(dim=-1)
        return (bins * self.rel_bin_centers).sum(dim=-1)

    def score(self, features: torch.Tensor) -> "DenseScores":
        """Dense scores for an episode's `(frames, 768)` embeddings, 32 windows per forward pass."""
        plan = WindowPlan(len(features))
        starts = torch.arange(plan.last_start + 1, device=features.device)
        steps = torch.arange(32, device=features.device) * plan.stride
        predictions = [self(features[chunk[:, None] + steps]) for chunk in starts.split(32)]
        return DenseScores(len(features), plan.stride, torch.cat(predictions).float().cpu().numpy())


class WindowPlan:
    """The stride of an episode's temporal windows and the last frame one can start at.

    Episodes shorter than 467 frames use the reference's reduced stride.
    """

    def __init__(self, frames: int):
        self.stride = max((frames - 2) // 31, 1) if frames <= 466 else 15
        self.last_start = frames - 31 * self.stride - 1


class DenseScores:
    """Each frame's signed velocity, averaged over the windows covering it, and its progress: the
    min-max normalized cumulative velocity, or None when that is constant."""

    def __init__(self, frames: int, stride: int, predictions: np.ndarray):
        """`predictions[start]` holds the 32 predictions of the window starting at frame `start`."""
        sums = np.zeros(frames, np.float64)
        self.support = np.zeros(frames, np.uint32)
        self.stride = stride
        span = 31 * stride
        steps = (predictions[:, 1:] - predictions[:, :-1]) * np.float32(31.0)
        for start, velocity in enumerate(steps * (np.float32(15.0) / np.float32(stride))):
            sums[start : start + span] += np.repeat(velocity, stride)
            self.support[start : start + span] += 1
        # A frame no window covers (always the last) keeps the velocity of the frame before it.
        previous = 0.0
        for frame in range(frames):
            if self.support[frame] > 0:
                previous = sums[frame] / self.support[frame]
            sums[frame] = previous
        self.velocity = sums.astype(np.float32)
        positions = np.cumsum(sums)
        low, high = positions.min(), positions.max()
        self.progress = (
            ((positions - low) / (high - low)).clip(0.0, 1.0).astype(np.float32) if high > low else None
        )

    def record_batch(self, timestamps_ns: np.ndarray) -> pa.RecordBatch:
        """One row per frame at the given presentation times, in `OUTPUT_SCHEMA`."""
        frames = len(self.velocity)
        return pa.record_batch(
            [
                pa.array(np.arange(frames, dtype=np.uint32)),
                pa.array(timestamps_ns),
                pa.array(self.velocity),
                pa.nulls(frames, pa.float32()) if self.progress is None else pa.array(self.progress),
                pa.array(np.full(frames, self.stride, np.uint32)),
                pa.array(self.support),
            ],
            schema=OUTPUT_SCHEMA,
        )
