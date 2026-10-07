import pytest
from video_service.testing import SyntheticEpisode


@pytest.fixture
def episode() -> SyntheticEpisode:
    return SyntheticEpisode(pictures=40, rate=30)
