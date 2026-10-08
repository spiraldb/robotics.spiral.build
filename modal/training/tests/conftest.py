from __future__ import annotations

from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from spiral_training_runtime.control import ControlService, RunControl
from spiral_training_runtime.publish import ModelPublisher
from spiral_training_runtime.testing import TOKEN, FakeCalls, LocalSync
from spiral_training_runtime.volume import TrainingVolume


@pytest.fixture
def volume(tmp_path: Path) -> TrainingVolume:
    return TrainingVolume(tmp_path / "volume", LocalSync())


@pytest.fixture
def calls() -> FakeCalls:
    return FakeCalls()


@pytest.fixture
def stop_flags() -> dict[str, bool]:
    return {}


@pytest.fixture
def models(tmp_path: Path) -> Path:
    """Where the models Volume is mounted."""
    return tmp_path / "models"


@pytest.fixture
def client(
    volume: TrainingVolume, calls: FakeCalls, stop_flags: dict[str, bool], models: Path
) -> TestClient:
    runs = RunControl(volume, calls, stop_flags)
    publisher = ModelPublisher(volume, models, LocalSync())
    return TestClient(ControlService(volume, runs, publisher, TOKEN).app())
