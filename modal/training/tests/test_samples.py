"""How the prepare step lays the training clock over the data."""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pyarrow as pa
import pytest

from spiral_training_runtime.errors import RuntimeRequestError
from spiral_training_runtime.samples import (
    STD_FLOOR,
    EpisodeSplit,
    EpisodeTicks,
    Normalization,
    TrainingSamples,
)
from spiral_training_runtime.streams import CameraStream, ValueStream
from spiral_training_runtime.testing import NANOSECONDS, SyntheticCamera, SyntheticRun, value_table
from spiral_training_runtime.video import CameraVideo


def test_ticks_cover_each_interval_up_to_but_excluding_its_end() -> None:
    ticks = EpisodeTicks.over(
        [(5 * NANOSECONDS, 6_500_000_000), (7 * NANOSECONDS, 9 * NANOSECONDS)], 10.0
    )

    assert len(ticks) == 15 + 20
    assert ticks.times_ns[0] == 5 * NANOSECONDS
    assert ticks.times_ns[14] == 6_400_000_000
    assert ticks.times_ns[15] == 7 * NANOSECONDS
    assert ticks.times_ns[-1] == 8_900_000_000
    assert set(ticks.interval_ends[:15]) == {15}
    assert set(ticks.interval_ends[15:]) == {35}


def test_a_rate_that_divides_no_nanosecond_still_stops_before_the_end() -> None:
    ticks = EpisodeTicks.over([(0, NANOSECONDS)], 30.0)

    assert len(ticks) == 30
    assert ticks.times_ns[-1] == 966_666_667


def test_values_hold_until_the_next_row_and_ticks_before_the_first_take_it() -> None:
    table = value_table(np.array([10, 20, 30]), np.array([[1.0, 10.0], [2.0, 20.0], [3.0, 30.0]]))
    stream = ValueStream(table)

    held = stream.held(np.array([5, 10, 19, 20, 35]))

    np.testing.assert_array_equal(held[:, 0], [1.0, 1.0, 1.0, 2.0, 3.0])
    assert stream.columns == 2


def test_a_row_with_a_null_value_is_skipped() -> None:
    table = pa.table(
        {
            "log_time_ns": pa.array([10, 20, 30], pa.int64()),
            "v0": pa.array([1.0, 2.0, 3.0], pa.float64()),
            "v1": pa.array([10.0, None, 30.0], pa.float64()),
        }
    )

    held = ValueStream(table).held(np.array([25]))

    np.testing.assert_array_equal(held, [[1.0, 10.0]])


def test_value_streams_must_be_in_log_time_order() -> None:
    with pytest.raises(RuntimeRequestError, match="log-time order"):
        ValueStream(value_table(np.array([20, 10]), np.zeros((2, 1))))


def test_each_tick_shows_the_latest_picture_presented_at_or_before_it() -> None:
    start = 1_000 * NANOSECONDS
    stream = CameraStream(SyntheticCamera.table(start, pictures=12))

    def presented(index: int) -> int:
        return start + index * NANOSECONDS // 15

    ticks = np.array(
        [
            presented(-5),  # before the first picture
            presented(0),
            presented(3) - 1,  # just before the fourth
            presented(3),
            presented(100),  # after the last
        ]
    )
    frames = np.zeros((len(ticks), 48, 64, 3), np.uint8)

    CameraVideo(stream).sample(ticks, frames)

    shown = frames.reshape(len(ticks), -1).mean(axis=1)
    expected = [SyntheticCamera.shade(index) for index in (0, 0, 2, 3, 11)]
    np.testing.assert_allclose(shown, expected, atol=4)


def test_standard_deviations_are_at_least_the_floor() -> None:
    states = np.array([[1.0, 0.0], [1.0, 2.0]])

    normalization = Normalization.over(states, states)

    np.testing.assert_allclose(normalization.state_std, [STD_FLOOR, 1.0])
    np.testing.assert_allclose(normalization.state_mean, [1.0, 1.0])
    assert normalization.action_std.dtype == np.float32


@pytest.mark.parametrize(
    ("episodes", "fraction", "held_out"),
    [
        (2, 0.5, 1),
        (10, 0.1, 1),
        (10, 0.25, 3),
        (10, 0.01, 1),
        (10, 0.0, 0),
        (1, 0.5, 0),
        (2, 0.9, 1),
    ],
)
def test_validation_holds_out_rounded_whole_episodes(
    episodes: int, fraction: float, held_out: int
) -> None:
    assert EpisodeSplit.held_out(episodes, fraction) == held_out


def test_the_validation_split_is_whole_episodes_chosen_by_the_seed() -> None:
    split = EpisodeSplit.choose([3, 4, 5, 6], 0.5, seed=7)

    assert len(split.validation) + len(split.train) == 18
    assert set(split.validation).isdisjoint(split.train)
    spans = [set(range(0, 3)), set(range(3, 7)), set(range(7, 12)), set(range(12, 18))]
    held_out = [span for span in spans if span <= set(split.validation)]
    assert sum(map(len, held_out)) == len(split.validation) and len(held_out) == 2
    np.testing.assert_array_equal(
        split.validation, EpisodeSplit.choose([3, 4, 5, 6], 0.5, 7).validation
    )


@pytest.fixture
def samples(volume, tmp_path: Path) -> TrainingSamples:
    run = SyntheticRun.from_fixture(volume)
    run.write()
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    return TrainingSamples(run.request, volume, scratch)


def test_the_fixture_prepares_55_ticks(samples: TrainingSamples) -> None:
    assert samples.ticks == 55
    assert samples.has_validation


def test_chunks_past_an_interval_end_are_padded_with_its_last_action(
    samples: TrainingSamples,
) -> None:
    # Episode 2's first interval holds ticks 20..34; chunk_size is 4.
    batch = samples.batch(np.array([32, 34, 20]))

    np.testing.assert_array_equal(
        batch.action_is_pad, [[False, False, False, True], [False, True, True, True], [False] * 4]
    )
    last_action = samples.batch(np.array([34])).actions[0, 0]
    np.testing.assert_array_equal(batch.actions[0, 2], last_action)
    np.testing.assert_array_equal(batch.actions[0, 3], last_action)
    np.testing.assert_array_equal(batch.actions[1, 1:], np.tile(last_action, (3, 1)))
    assert not np.array_equal(batch.actions[2, 1], batch.actions[2, 0])
    assert batch.images.shape == (3, 2, 48, 64, 3)
    assert batch.states.dtype == np.float32
