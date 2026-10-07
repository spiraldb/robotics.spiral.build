import pytest

from video_service.episode import EpisodeVideo, RejectedVideo


def test_frames_come_in_display_order_with_their_packet_times(episode):
    assert episode.times_ns != sorted(episode.times_ns)
    frames = list(EpisodeVideo(episode.payloads, episode.times_ns).frames())
    assert [frame.pts for frame in frames] == sorted(episode.times_ns)
    # The grey level the encoder painted rises picture by picture.
    levels = [int(frame.to_ndarray(format="rgb24").mean()) for frame in frames]
    assert levels == sorted(levels)


def test_undecodable_packets_are_rejected():
    with pytest.raises(RejectedVideo):
        list(EpisodeVideo([b"\x00\x00\x01\x65garbage"] * 3, [0, 1, 2]).frames())

