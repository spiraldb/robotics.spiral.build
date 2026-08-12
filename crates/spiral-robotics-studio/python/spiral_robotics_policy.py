"""Vision-conditioned Action Chunking Transformer policy and objective."""

from __future__ import annotations

import math
from typing import Any

import torch
import torch.nn.functional as functional
from torch import nn


RESNET18_WEIGHTS_URL = "https://download.pytorch.org/models/resnet18-f37072fd.pth"


class FrozenBatchNorm2d(nn.Module):
    """Reference DETR/ACT batch normalization with immutable ImageNet stats."""

    def __init__(self, channels: int) -> None:
        super().__init__()
        self.register_buffer("weight", torch.ones(channels))
        self.register_buffer("bias", torch.zeros(channels))
        self.register_buffer("running_mean", torch.zeros(channels))
        self.register_buffer("running_var", torch.ones(channels))

    def forward(self, value: torch.Tensor) -> torch.Tensor:
        weight = self.weight.reshape(1, -1, 1, 1)
        bias = self.bias.reshape(1, -1, 1, 1)
        mean = self.running_mean.reshape(1, -1, 1, 1)
        variance = self.running_var.reshape(1, -1, 1, 1)
        scale = weight * (variance + 1.0e-5).rsqrt()
        return value * scale + (bias - mean * scale)


class BasicBlock(nn.Module):
    expansion = 1

    def __init__(self, in_channels: int, channels: int, stride: int = 1) -> None:
        super().__init__()
        self.conv1 = nn.Conv2d(in_channels, channels, 3, stride=stride, padding=1, bias=False)
        self.bn1 = FrozenBatchNorm2d(channels)
        self.conv2 = nn.Conv2d(channels, channels, 3, padding=1, bias=False)
        self.bn2 = FrozenBatchNorm2d(channels)
        self.downsample = (
            nn.Sequential(
                nn.Conv2d(in_channels, channels, 1, stride=stride, bias=False),
                FrozenBatchNorm2d(channels),
            )
            if stride != 1 or in_channels != channels
            else nn.Identity()
        )

    def forward(self, value: torch.Tensor) -> torch.Tensor:
        residual = self.downsample(value)
        value = functional.relu(self.bn1(self.conv1(value)), inplace=True)
        value = self.bn2(self.conv2(value))
        return functional.relu(value + residual, inplace=True)


class ResNet18Backbone(nn.Module):
    """ResNet-18 spatial feature extractor."""

    output_channels = 512

    def __init__(self, pretrained: bool) -> None:
        super().__init__()
        self.conv1 = nn.Conv2d(3, 64, 7, stride=2, padding=3, bias=False)
        self.bn1 = FrozenBatchNorm2d(64)
        self.relu = nn.ReLU(inplace=True)
        self.maxpool = nn.MaxPool2d(3, stride=2, padding=1)
        self.layer1 = self._layer(64, 64, 2, 1)
        self.layer2 = self._layer(64, 128, 2, 2)
        self.layer3 = self._layer(128, 256, 2, 2)
        self.layer4 = self._layer(256, 512, 2, 2)
        if pretrained:
            try:
                state = torch.hub.load_state_dict_from_url(
                    RESNET18_WEIGHTS_URL,
                    map_location="cpu",
                    check_hash=True,
                    progress=True,
                )
            except Exception as error:
                raise RuntimeError(
                    "ImageNet ResNet-18 weights are required for ACT; allow the one-time "
                    "download or pre-populate the Torch checkpoints cache"
                ) from error
            state = {
                name: value
                for name, value in state.items()
                if not name.startswith("fc.") and not name.endswith("num_batches_tracked")
            }
            missing, unexpected = self.load_state_dict(state, strict=False)
            if missing or unexpected:
                raise RuntimeError(
                    f"pretrained ResNet-18 contract mismatch: missing={missing}, "
                    f"unexpected={unexpected}"
                )

    @staticmethod
    def _layer(in_channels: int, channels: int, blocks: int, stride: int) -> nn.Sequential:
        layers: list[nn.Module] = [BasicBlock(in_channels, channels, stride)]
        layers.extend(BasicBlock(channels, channels) for _ in range(blocks - 1))
        return nn.Sequential(*layers)

    def forward(self, image: torch.Tensor) -> torch.Tensor:
        image = self.maxpool(self.relu(self.bn1(self.conv1(image))))
        image = self.layer1(image)
        image = self.layer2(image)
        image = self.layer3(image)
        return self.layer4(image)


def _position_2d(
    height: int,
    width: int,
    dim: int,
    device: torch.device,
    dtype: torch.dtype,
) -> torch.Tensor:
    if dim % 4 != 0:
        raise ValueError("ACT hidden_dim must be divisible by four")
    quarter = dim // 4
    scale = torch.exp(
        torch.arange(quarter, device=device, dtype=dtype)
        * (-math.log(10_000.0) / max(quarter - 1, 1))
    )
    y = torch.arange(height, device=device, dtype=dtype).unsqueeze(1) * scale.unsqueeze(0)
    x = torch.arange(width, device=device, dtype=dtype).unsqueeze(1) * scale.unsqueeze(0)
    y = torch.cat((y.sin(), y.cos()), dim=1)[:, None, :].expand(height, width, -1)
    x = torch.cat((x.sin(), x.cos()), dim=1)[None, :, :].expand(height, width, -1)
    return torch.cat((y, x), dim=2).reshape(height * width, dim)


class ActPolicy(nn.Module):
    """Image-conditioned Action Chunking Transformer with a CVAE latent."""

    def __init__(
        self,
        state_dim: int,
        action_dim: int,
        camera_count: int,
        chunk_size: int,
        config: dict[str, Any],
    ) -> None:
        super().__init__()
        hidden = int(config["hidden_dim"])
        heads = int(config["attention_heads"])
        feedforward = int(config["feedforward_dim"])
        self.state_dim = state_dim
        self.action_dim = action_dim
        self.camera_count = camera_count
        self.chunk_size = chunk_size
        self.hidden_dim = hidden

        self.backbone = ResNet18Backbone(
            pretrained=bool(config.get("pretrained_backbone", True))
        )
        self.image_projection = nn.Conv2d(self.backbone.output_channels, hidden, 1)
        self.camera_embedding = nn.Embedding(camera_count, hidden)
        self.state_projection = nn.Linear(state_dim, hidden)
        self.latent_projection = nn.Linear(int(config["latent_dim"]), hidden)
        self.memory_prefix_position = nn.Embedding(2, hidden)

        cvae_layer = nn.TransformerEncoderLayer(
            d_model=hidden,
            nhead=heads,
            dim_feedforward=feedforward,
            dropout=0.1,
            activation="gelu",
            batch_first=True,
            norm_first=True,
        )
        self.cvae_encoder = nn.TransformerEncoder(
            cvae_layer,
            num_layers=int(config["encoder_layers"]),
            enable_nested_tensor=False,
        )
        self.cvae_cls = nn.Parameter(torch.zeros(1, 1, hidden))
        self.cvae_state = nn.Linear(state_dim, hidden)
        self.cvae_action = nn.Linear(action_dim, hidden)
        self.cvae_position = nn.Embedding(chunk_size + 2, hidden)
        self.latent_stats = nn.Linear(hidden, int(config["latent_dim"]) * 2)

        encoder_layer = nn.TransformerEncoderLayer(
            d_model=hidden,
            nhead=heads,
            dim_feedforward=feedforward,
            dropout=0.1,
            activation="gelu",
            batch_first=True,
            norm_first=True,
        )
        self.encoder = nn.TransformerEncoder(
            encoder_layer,
            num_layers=int(config["encoder_layers"]),
            enable_nested_tensor=False,
        )
        decoder_layer = nn.TransformerDecoderLayer(
            d_model=hidden,
            nhead=heads,
            dim_feedforward=feedforward,
            dropout=0.1,
            activation="gelu",
            batch_first=True,
            norm_first=True,
        )
        self.decoder = nn.TransformerDecoder(
            decoder_layer, num_layers=int(config["decoder_layers"])
        )
        self.action_queries = nn.Embedding(chunk_size, hidden)
        self.action_head = nn.Linear(hidden, action_dim)
        self.pad_head = nn.Linear(hidden, 1)
        nn.init.normal_(self.camera_embedding.weight, std=0.02)
        nn.init.normal_(self.memory_prefix_position.weight, std=0.02)
        nn.init.normal_(self.action_queries.weight, std=0.02)
        nn.init.normal_(self.cvae_position.weight, std=0.02)

    def _latent(
        self,
        state: torch.Tensor,
        actions: torch.Tensor | None,
        action_is_pad: torch.Tensor | None,
    ) -> tuple[torch.Tensor, torch.Tensor | None, torch.Tensor | None]:
        batch = state.shape[0]
        if actions is None:
            latent_dim = self.latent_stats.out_features // 2
            return state.new_zeros((batch, latent_dim)), None, None
        cls = self.cvae_cls.expand(batch, -1, -1)
        tokens = torch.cat(
            (cls, self.cvae_state(state).unsqueeze(1), self.cvae_action(actions)), dim=1
        )
        tokens = tokens + self.cvae_position.weight.unsqueeze(0)
        prefix_mask = torch.zeros((batch, 2), dtype=torch.bool, device=state.device)
        padding = (
            torch.cat((prefix_mask, action_is_pad), dim=1)
            if action_is_pad is not None
            else torch.zeros(
                (batch, self.chunk_size + 2), dtype=torch.bool, device=state.device
            )
        )
        encoded = self.cvae_encoder(tokens, src_key_padding_mask=padding)[:, 0]
        mean, log_variance = self.latent_stats(encoded).chunk(2, dim=-1)
        latent = mean + (0.5 * log_variance).exp() * torch.randn_like(mean)
        return latent, mean, log_variance

    def forward(
        self,
        state: torch.Tensor,
        images: torch.Tensor,
        actions: torch.Tensor | None = None,
        action_is_pad: torch.Tensor | None = None,
    ) -> tuple[torch.Tensor, torch.Tensor, torch.Tensor | None, torch.Tensor | None]:
        if images.ndim != 5 or images.shape[1] != self.camera_count:
            raise ValueError("ACT images must have shape [batch, cameras, 3, height, width]")
        batch, cameras, channels, height, width = images.shape
        features = self.backbone(images.reshape(batch * cameras, channels, height, width))
        features = self.image_projection(features)
        _, _, feature_height, feature_width = features.shape
        features = features.flatten(2).transpose(1, 2).reshape(
            batch, cameras, -1, self.hidden_dim
        )
        spatial_position = _position_2d(
            feature_height, feature_width, self.hidden_dim, images.device, features.dtype
        )
        camera_position = self.camera_embedding.weight[:, None, :] + spatial_position[None, :, :]
        image_tokens = (features + camera_position[None, :, :, :]).reshape(
            batch, -1, self.hidden_dim
        )

        latent, mean, log_variance = self._latent(state, actions, action_is_pad)
        prefix = torch.stack((self.latent_projection(latent), self.state_projection(state)), dim=1)
        prefix = prefix + self.memory_prefix_position.weight.unsqueeze(0)
        memory = self.encoder(torch.cat((prefix, image_tokens), dim=1))
        queries = self.action_queries.weight.unsqueeze(0).expand(batch, -1, -1)
        decoded = self.decoder(queries, memory)
        return self.action_head(decoded), self.pad_head(decoded).squeeze(-1), mean, log_variance


def act_loss(
    predicted: torch.Tensor,
    predicted_pad: torch.Tensor,
    target: torch.Tensor,
    target_pad: torch.Tensor,
    mean: torch.Tensor | None,
    log_variance: torch.Tensor | None,
    kl_weight: float,
) -> tuple[torch.Tensor, dict[str, float]]:
    keep = (~target_pad).unsqueeze(-1)
    action_values = keep.sum().clamp_min(1) * target.shape[-1]
    l1 = ((predicted - target).abs() * keep).sum() / action_values
    pad = functional.binary_cross_entropy_with_logits(
        predicted_pad, target_pad.to(torch.float32)
    )
    if mean is None or log_variance is None:
        kl = predicted.new_zeros(())
    else:
        kl = (
            -0.5 * (1.0 + log_variance - mean.square() - log_variance.exp())
        ).sum(-1).mean()
    loss = l1 + float(kl_weight) * kl
    return loss, {
        "l1": float(l1.detach()),
        "pad": float(pad.detach()),
        "kl": float(kl.detach()),
    }


def self_test() -> None:
    config = {
        "hidden_dim": 64,
        "latent_dim": 8,
        "encoder_layers": 1,
        "decoder_layers": 1,
        "attention_heads": 8,
        "feedforward_dim": 128,
        "pretrained_backbone": False,
    }
    model = ActPolicy(6, 6, 2, 4, config)
    state = torch.zeros((2, 6))
    images = torch.zeros((2, 2, 3, 64, 64))
    actions = torch.zeros((2, 4, 6))
    padding = torch.tensor([[False, False, False, False], [False, True, True, True]])
    predicted, predicted_pad, mean, log_variance = model(
        state, images, actions, padding
    )
    assert predicted.shape == (2, 4, 6)
    assert predicted_pad.shape == (2, 4)
    assert mean is not None and mean.shape == (2, 8)
    loss, parts = act_loss(
        predicted, predicted_pad, actions, padding, mean, log_variance, 10.0
    )
    assert loss.isfinite() and set(parts) == {"l1", "pad", "kl"}
    reference_loss, _ = act_loss(
        torch.zeros((1, 2, 6)),
        torch.zeros((1, 2)),
        torch.ones((1, 2, 6)),
        torch.zeros((1, 2), dtype=torch.bool),
        torch.zeros((1, 8)),
        torch.zeros((1, 8)),
        10.0,
    )
    assert torch.isclose(reference_loss, torch.tensor(1.0))


__all__ = ["ActPolicy", "act_loss", "self_test"]
