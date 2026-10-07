"""The config of an ACT checkpoint the policy runtime serves."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from .errors import RuntimeRequestError


@dataclass(frozen=True)
class PolicyConfig:
    state_name: str
    state_dimensions: int
    camera_streams: tuple[str, ...]
    image_height: int
    image_width: int
    action_name: str
    action_dimensions: int
    chunk_size: int
    action_steps: int
    hidden_dimensions: int
    latent_dimensions: int
    attention_heads: int
    feedforward_dimensions: int
    encoder_layers: int
    vae_encoder_layers: int
    decoder_layers: int
    pre_norm: bool
    use_vae: bool
    dropout: float

    @classmethod
    def from_json(cls, raw: dict[str, Any]) -> PolicyConfig:
        if raw.get("type") != "act":
            raise RuntimeRequestError("policy artifact is not an ACT config", 422)
        if raw.get("n_obs_steps") != 1:
            raise RuntimeRequestError("ACT policy artifacts must use one observation step", 422)
        if raw.get("vision_backbone") != "resnet18":
            raise RuntimeRequestError("only resnet18 ACT policy artifacts are supported", 422)
        if raw.get("replace_final_stride_with_dilation", False):
            raise RuntimeRequestError("dilated ACT backbones are unsupported", 422)

        inputs = _mapping(raw, "input_features")
        outputs = _mapping(raw, "output_features")
        state_entries = [
            (name, feature)
            for name, feature in inputs.items()
            if isinstance(feature, dict) and feature.get("type") == "STATE"
        ]
        action_entries = [
            (name, feature)
            for name, feature in outputs.items()
            if isinstance(feature, dict) and feature.get("type") == "ACTION"
        ]
        visual_entries = {
            name: feature
            for name, feature in inputs.items()
            if isinstance(feature, dict) and feature.get("type") == "VISUAL"
        }
        if len(state_entries) != 1 or len(action_entries) != 1 or not visual_entries:
            raise RuntimeRequestError(
                "ACT requires one state, one action, and at least one visual feature", 422
            )
        camera_order = raw.get("spiral_camera_order") or sorted(visual_entries)
        if not isinstance(camera_order, list) or set(camera_order) != set(visual_entries):
            raise RuntimeRequestError(
                "spiral_camera_order must list every visual feature exactly once", 422
            )
        camera_shapes = [_shape(visual_entries[name], name, rank=3) for name in camera_order]
        if any(shape[0] != 3 for shape in camera_shapes):
            raise RuntimeRequestError("ACT visual features must have three RGB channels", 422)
        sizes = {(shape[1], shape[2]) for shape in camera_shapes}
        if len(sizes) != 1:
            raise RuntimeRequestError("all ACT cameras must use the same image dimensions", 422)
        image_height, image_width = next(iter(sizes))
        state_name, state_feature = state_entries[0]
        action_name, action_feature = action_entries[0]
        state_shape = _shape(state_feature, state_name, rank=1)
        action_shape = _shape(action_feature, action_name, rank=1)
        config = cls(
            state_name=state_name,
            state_dimensions=state_shape[0],
            camera_streams=tuple(camera_order),
            image_height=image_height,
            image_width=image_width,
            action_name=action_name,
            action_dimensions=action_shape[0],
            chunk_size=_positive_int(raw, "chunk_size"),
            action_steps=_positive_int(raw, "n_action_steps"),
            hidden_dimensions=_positive_int(raw, "dim_model"),
            latent_dimensions=_positive_int(raw, "latent_dim"),
            attention_heads=_positive_int(raw, "n_heads"),
            feedforward_dimensions=_positive_int(raw, "dim_feedforward"),
            encoder_layers=_positive_int(raw, "n_encoder_layers"),
            vae_encoder_layers=int(raw.get("n_vae_encoder_layers", 4)),
            decoder_layers=_positive_int(raw, "n_decoder_layers"),
            pre_norm=bool(raw.get("pre_norm", False)),
            use_vae=bool(raw.get("use_vae", True)),
            dropout=float(raw.get("dropout", 0.1)),
        )
        if config.action_steps > config.chunk_size:
            raise RuntimeRequestError("ACT action steps cannot exceed the chunk size", 422)
        if config.hidden_dimensions % config.attention_heads != 0:
            raise RuntimeRequestError(
                "ACT hidden dimensions must be divisible by attention heads", 422
            )
        if config.vae_encoder_layers <= 0:
            raise RuntimeRequestError("ACT VAE encoder layers must be positive", 422)
        return config


def _mapping(source: dict[str, Any], name: str) -> dict[str, Any]:
    value = source.get(name)
    if not isinstance(value, dict):
        raise RuntimeRequestError(f"{name} must be an object", 422)
    return value


def _positive_int(source: dict[str, Any], name: str) -> int:
    value = source.get(name)
    if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
        raise RuntimeRequestError(f"{name} must be a positive integer", 422)
    return value


def _shape(feature: dict[str, Any], name: str, rank: int) -> tuple[int, ...]:
    shape = feature.get("shape")
    if (
        not isinstance(shape, list)
        or len(shape) != rank
        or any(
            isinstance(value, bool) or not isinstance(value, int) or value <= 0 for value in shape
        )
    ):
        raise RuntimeRequestError(f"feature {name!r} must have a positive rank-{rank} shape", 422)
    return tuple(shape)
