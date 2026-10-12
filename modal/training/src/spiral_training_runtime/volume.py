"""The Volume the control endpoint and the job share: where each file lives, and when each side
commits and reloads.

    streams/<key[0..2]>/<key>.arrow                      one uploaded stream
    runs/<project>/<run_id>/request.json                 the run request, as posted
    runs/<project>/<run_id>/call.json                    {"function_call_id": "…"}
    runs/<project>/<run_id>/status.json                  where the run is
    runs/<project>/<run_id>/history.json                 metrics and log tail
    runs/<project>/<run_id>/checkpoints/<step>/{config.json,model.safetensors}
    runs/<project>/<run_id>/resume.pt                    latest model + optimizer state
"""

from __future__ import annotations

import json
import shutil
import threading
from collections.abc import Iterable, Iterator
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Protocol

from .report import RunStatus
from .request import RunKey


class VolumeSync(Protocol):
    """How a mount meets other containers' writes; a `modal.Volume` is one."""

    def commit(self) -> None: ...

    def reload(self) -> None: ...


class TrainingVolume:
    def __init__(self, root: Path, sync: VolumeSync) -> None:
        self._root = root
        self._sync = sync
        # Modal fails a reload or a commit while any file on the mount is open, so file access,
        # reloads, and commits take turns.
        self._lock = threading.Lock()

    @contextmanager
    def reading(self) -> Iterator[None]:
        """The mount held still, showing what other containers last committed."""
        with self._lock:
            self._sync.reload()
            yield

    @contextmanager
    def writing(self) -> Iterator[None]:
        """The mount held still; what is written inside is committed on the way out."""
        with self._lock:
            yield
            self._sync.commit()

    def stream(self, key: str) -> Path:
        return self._root / "streams" / key[:2] / f"{key}.arrow"

    def missing_streams(self, keys: Iterable[str]) -> list[str]:
        with self.reading():
            return [key for key in keys if not self.stream(key).is_file()]

    def store_stream(self, key: str, upload: Path) -> None:
        """Copies a complete upload under its key and commits it.

        Uploads are received off the mount: an upload in progress would hold a file open and
        block every reload until it finished. The copy lands whole, since Modal also commits in
        the background.
        """
        destination = self.stream(key)
        with self.writing():
            destination.parent.mkdir(parents=True, exist_ok=True)
            partial = destination.with_name(f"{destination.name}.partial")
            shutil.copyfile(upload, partial)
            partial.replace(destination)

    def run(self, key: RunKey) -> RunFiles:
        return RunFiles(self._root / "runs" / key.project / key.run_id)


class RunFiles:
    """One run's files; read them inside the Volume's `reading` and write them inside `writing`."""

    def __init__(self, directory: Path) -> None:
        self._directory = directory

    def exists(self) -> bool:
        """Whether a run was posted here."""
        return (self._directory / "request.json").is_file()

    def checkpoint(self, step: int) -> Path:
        return self._directory / "checkpoints" / str(step)

    def keep_checkpoints(self, steps: Iterable[int]) -> None:
        """Deletes every checkpoint but those of `steps`: one a best checkpoint replaced, and any an
        attempt saved after the state its retry resumed from."""
        kept = {str(step) for step in steps}
        directory = self._directory / "checkpoints"
        if not directory.is_dir():
            return
        for checkpoint in directory.iterdir():
            if checkpoint.name not in kept:
                shutil.rmtree(checkpoint, ignore_errors=True)

    def read_request(self) -> bytes | None:
        return self._read("request.json")

    def write_request(self, body: bytes) -> None:
        self._write("request.json", body)

    def read_call_id(self) -> str | None:
        call = self._read("call.json")
        return None if call is None else json.loads(call)["function_call_id"]

    def write_call_id(self, call_id: str) -> None:
        self._write("call.json", json.dumps({"function_call_id": call_id}).encode())

    def read_status(self) -> RunStatus | None:
        status = self._read("status.json")
        return None if status is None else RunStatus.model_validate_json(status)

    def write_status(self, status: RunStatus) -> None:
        self._write("status.json", status.model_dump_json().encode())

    def read_history(self) -> dict[str, Any] | None:
        history = self._read("history.json")
        return None if history is None else json.loads(history)

    def write_history(self, history: dict[str, Any]) -> None:
        self._write("history.json", json.dumps(history).encode())

    def read_resume(self) -> bytes | None:
        return self._read("resume.pt")

    def write_resume(self, state: bytes) -> None:
        self._write("resume.pt", state)

    def delete(self) -> None:
        shutil.rmtree(self._directory, ignore_errors=True)

    def _read(self, name: str) -> bytes | None:
        path = self._directory / name
        return path.read_bytes() if path.is_file() else None

    def _write(self, name: str, data: bytes) -> None:
        """Replaces a file whole: Modal also commits in the background, at any moment."""
        self._directory.mkdir(parents=True, exist_ok=True)
        partial = self._directory / f"{name}.partial"
        partial.write_bytes(data)
        partial.replace(self._directory / name)
