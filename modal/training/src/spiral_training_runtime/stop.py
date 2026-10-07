"""Stopping a run: the control endpoint raises its flag, and its job reads it between steps."""

from __future__ import annotations

import math
import time
from typing import Protocol

from .request import RunKey

# A flag read is a network call, so a job reads it at most this often rather than every step.
STOP_POLL_SECONDS = 2.0


class StopFlags(Protocol):
    """Runs asked to stop, by `RunKey.flag`; a `modal.Dict` is one."""

    def __setitem__(self, flag: str, requested: bool) -> None: ...

    def get(self, flag: str, default: bool = False) -> bool: ...

    def pop(self, flag: str, default: bool | None = None) -> bool | None: ...


class StopRequest:
    """Whether a job's run was asked to stop. Once asked, it stays asked."""

    def __init__(self, flags: StopFlags, key: RunKey) -> None:
        self._flags = flags
        self._flag = key.flag
        self._checked_at = -math.inf
        self._requested = False

    def requested(self) -> bool:
        now = time.monotonic()
        if not self._requested and now - self._checked_at >= STOP_POLL_SECONDS:
            self._checked_at = now
            self._requested = bool(self._flags.get(self._flag, False))
        return self._requested
