from __future__ import annotations

from typing import Any

from .errors import RuntimeRequestError


class McapTrainingDataset:
    """The native MCAP training source awaits Spiral's packet-to-image interface."""

    def __init__(self, request: dict[str, Any]) -> None:
        raise RuntimeRequestError(
            "MCAP camera training is unavailable until Spiral supports decoding "
            "MCAP video into training images.",
            503,
        )
