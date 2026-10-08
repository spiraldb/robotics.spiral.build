import pytest

from video_service.episode import EpisodeVideo, RejectedVideo

from object_tracking import video
from object_tracking.video import SampledVideo, SampleRate

FIVE_A_SECOND = SampleRate("5")


def test_samples_the_first_picture_and_then_one_every_200_ms_in_display_order(episode):
    pictures = SampledVideo(EpisodeVideo(episode.payloads, episode.times_ns), FIVE_A_SECOND).pictures()
    assert [picture.time_ns for picture in pictures] == [index * 200_000_000 for index in range(7)]
    assert pictures[0].rgb.shape == (48, 64, 3)
    # Display order: the grey level the encoder painted rises picture by picture.
    levels = [int(picture.rgb.mean()) for picture in pictures]
    assert levels == sorted(levels)


def test_refuses_an_episode_longer_than_it_samples(episode, monkeypatch):
    monkeypatch.setattr(video, "MAX_PICTURES", 3)
    with pytest.raises(RejectedVideo, match="longer than 3 sampled pictures"):
        SampledVideo(EpisodeVideo(episode.payloads, episode.times_ns), FIVE_A_SECOND).pictures()


def test_refuses_an_undecodable_or_empty_video():
    with pytest.raises(RejectedVideo):
        SampledVideo(EpisodeVideo([b"\x00\x00\x01\x65garbage"], [0]), FIVE_A_SECOND).pictures()
    with pytest.raises(RejectedVideo, match="no picture"):
        SampledVideo(EpisodeVideo([], []), FIVE_A_SECOND).pictures()


def test_keeps_every_sixth_picture_when_30_hz_times_are_truncated(episode):
    # Recorded 30 Hz times step by 33,333,333 ns, which puts every sixth picture a few nanoseconds
    # before its 200 ms.
    truncated = [time_ns // 33_333_333 * 33_333_333 for time_ns in episode.times_ns]
    pictures = SampledVideo(EpisodeVideo(episode.payloads, truncated), FIVE_A_SECOND).pictures()
    assert [picture.time_ns for picture in pictures] == [index * 6 * 33_333_333 for index in range(7)]


def test_samples_every_picture_at_the_cameras_own_rate(episode):
    pictures = SampledVideo(EpisodeVideo(episode.payloads, episode.times_ns), SampleRate("30")).pictures()
    assert len(pictures) == 40


@pytest.mark.parametrize("rate", [None, "", "0", "-1", "30.5", "nan", "fast"])
def test_refuses_a_rate_it_cannot_sample_at(rate):
    with pytest.raises(RejectedVideo, match="sample rate"):
        SampleRate(rate)
