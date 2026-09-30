"""Where one request's time goes, stage by stage."""

import time
from collections.abc import Iterable, Iterator
from contextlib import contextmanager


class StageTimes:
    """Seconds one request spent in each named stage, in the order the stages were first entered."""

    def __init__(self):
        self._seconds: dict[str, float] = {}

    @contextmanager
    def stage(self, name: str) -> Iterator[None]:
        started = time.perf_counter()
        try:
            yield
        finally:
            self._seconds[name] = self._seconds.get(name, 0.0) + time.perf_counter() - started

    def timed[T](self, name: str, items: Iterable[T]) -> Iterator[T]:
        """`items`, with the time spent producing each one added to stage `name`."""
        iterator = iter(items)
        while True:
            with self.stage(name):
                try:
                    item = next(iterator)
                except StopIteration:
                    return
            yield item

    def server_timing(self) -> str:
        """The stages as a `Server-Timing` header value, in milliseconds."""
        return ", ".join(f"{name};dur={1000 * seconds:.1f}" for name, seconds in self._seconds.items())
