from __future__ import annotations

from spiral_training_runtime.__main__ import _is_loopback_host


def test_loopback_host_detection_fails_closed() -> None:
    assert _is_loopback_host("127.0.0.1")
    assert _is_loopback_host("::1")
    assert _is_loopback_host("[::1]")
    assert _is_loopback_host("localhost")
    assert not _is_loopback_host("0.0.0.0")
    assert not _is_loopback_host("::")
    assert not _is_loopback_host("runtime.internal")
