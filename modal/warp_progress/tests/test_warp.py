import numpy as np
import pytest
import torch

from video_service.episode import EpisodeVideo, RejectedVideo

from warp_progress.warp import (
    FRAME_PERIOD_NS,
    IMAGENET_MEAN,
    IMAGENET_STD,
    DenseScores,
    PictureEncoder,
    PresentationTimes,
    SquashPreprocessing,
    WindowPlan,
)


def test_pictures_come_in_display_order_as_rgb_with_their_packet_times(episode):
    pictures = list(PictureEncoder._pictures(EpisodeVideo(episode.payloads, episode.times_ns)))
    assert [time_ns for time_ns, _ in pictures] == sorted(episode.times_ns)
    assert pictures[0][1].shape == (48, 64, 3)


def test_presentation_times_need_33_pictures_on_a_30_hz_clock():
    on_clock = [index * FRAME_PERIOD_NS for index in range(33)]
    PresentationTimes([time + 999_999 * (index % 2) for index, time in enumerate(on_clock)])
    for times in (on_clock[:32], on_clock[:20] + [time + 1_000_001 for time in on_clock[20:]]):
        with pytest.raises(RejectedVideo):
            PresentationTimes(times)
    with pytest.raises(RejectedVideo):
        PresentationTimes([time - 1 for time in on_clock])


def test_window_plan_reduces_the_stride_below_467_frames():
    assert [WindowPlan(frames).stride for frames in (33, 466, 467)] == [1, 14, 15]


def test_dense_scores_keep_signed_velocity_and_null_constant_progress():
    rising = np.tile(np.arange(32, dtype=np.float32) / 31, (2, 1))
    scores = DenseScores(33, 1, rising)
    assert np.allclose(scores.velocity, 15.0, atol=1e-4)
    assert (scores.progress[0], scores.progress[-1]) == (0.0, 1.0)
    assert scores.support[32] == 0
    assert np.allclose(DenseScores(33, 1, -rising).velocity, -15.0, atol=1e-4)
    assert DenseScores(33, 1, np.zeros((1, 32), np.float32)).progress is None


DEVICES = ["cpu"] + ["mps"] * torch.backends.mps.is_available() + ["cuda"] * torch.cuda.is_available()


def normalized(value: np.ndarray) -> np.ndarray:
    return ((value.astype(np.float32) / 255 - IMAGENET_MEAN) / IMAGENET_STD).transpose(2, 0, 1)


def squash(pictures: np.ndarray, device: str) -> np.ndarray:
    squashing = SquashPreprocessing(pictures.shape[1:3], torch.device(device))
    return squashing.apply(torch.from_numpy(pictures).to(device)).cpu().numpy()


def rust_squash(rgb: np.ndarray) -> np.ndarray:
    """warp.rs's SquashPreprocessing, in int64 and float64 numpy: the oracle."""

    def coordinates(source: int):
        weights = []
        for dest in range(224):
            position = (dest + 0.5) * source / 224.0 - 0.5
            origin = int(np.floor(position))
            fraction = 0.0 if origin < 0 or origin >= source - 1 else position - origin
            first = min(max(origin, 0), source - 1)
            weights.append((first, min(first + 1, source - 1), round((1.0 - fraction) * 2048.0), round(fraction * 2048.0)))
        return [np.array(column, np.int64) for column in zip(*weights)]

    (x0, x1, a0, a1), (y0, y1, b0, b1) = coordinates(rgb.shape[1]), coordinates(rgb.shape[0])
    pixel = rgb.astype(np.int64)
    top = pixel[y0][:, x0] * a0[:, None] + pixel[y0][:, x1] * a1[:, None]
    bottom = pixel[y1][:, x0] * a0[:, None] + pixel[y1][:, x1] * a1[:, None]
    value = ((top * b0[:, None, None] + bottom * b1[:, None, None] + (1 << 21)) >> 22).clip(0, 255)
    return normalized(value)


@pytest.mark.parametrize("device", DEVICES)
def test_squash_keeps_a_224_picture_and_averages_2x2_blocks_of_a_448_picture(device):
    rng = np.random.default_rng(0)
    rgb = rng.integers(0, 256, (224, 224, 3), dtype=np.uint8)
    assert np.array_equal(squash(rgb[None], device)[0], normalized(rgb))
    rgb = rng.integers(0, 256, (448, 448, 3), dtype=np.uint8)
    blocks = rgb.astype(np.int64).reshape(224, 2, 224, 2, 3).sum(axis=(1, 3))
    assert np.array_equal(squash(rgb[None], device)[0], normalized((blocks + 2) // 4))


@pytest.mark.parametrize("device", DEVICES)
@pytest.mark.parametrize("size", [(480, 640), (1080, 1920), (131, 97), (7, 5), (224, 225)])
def test_squash_is_bit_identical_to_the_rust_maths(device, size):
    pictures = np.random.default_rng(1).integers(0, 256, (3, *size, 3), dtype=np.uint8)
    pictures[:, :8] = 255
    squashed = squash(pictures, device)
    assert all(np.array_equal(squashed[index], rust_squash(picture)) for index, picture in enumerate(pictures))
