#!/usr/bin/env python3
"""Build a deterministic Studio ACT artifact for native parity testing."""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import torch
from safetensors.torch import save_file

from spiral_robotics_policy import ActPolicy


CAMERA = "follower.camera.wrist"
STATE = [0.1, -0.2, 0.3, -0.4, 0.5, -0.6]
MEAN = torch.tensor([0.485, 0.456, 0.406], dtype=torch.float32).reshape(3, 1, 1)
STD = torch.tensor([0.229, 0.224, 0.225], dtype=torch.float32).reshape(3, 1, 1)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)

    torch.manual_seed(7)
    act = {
        "hidden_dim": 64,
        "latent_dim": 8,
        "encoder_layers": 1,
        "decoder_layers": 1,
        "attention_heads": 8,
        "feedforward_dim": 128,
        "pretrained_backbone": False,
    }
    policy = ActPolicy(6, 6, 1, 4, act).eval()
    state = torch.tensor([STATE], dtype=torch.float32)
    raw_image = torch.zeros((1, 1, 3, 64, 64), dtype=torch.float32)
    images = (raw_image - MEAN.reshape(1, 1, 3, 1, 1)) / STD.reshape(
        1, 1, 3, 1, 1
    )
    with torch.inference_mode():
        expected = policy(state, images)[0]

    config = {
        "type": "act",
        "n_obs_steps": 1,
        "normalization_mapping": {
            "VISUAL": "MEAN_STD",
            "STATE": "MEAN_STD",
            "ACTION": "MEAN_STD",
        },
        "input_features": {
            "observation.state": {"type": "STATE", "shape": [6]},
            CAMERA: {"type": "VISUAL", "shape": [3, 64, 64]},
        },
        "output_features": {"action": {"type": "ACTION", "shape": [6]}},
        "chunk_size": 4,
        "n_action_steps": 4,
        "vision_backbone": "resnet18",
        "replace_final_stride_with_dilation": False,
        "pre_norm": False,
        "dim_model": 64,
        "n_heads": 8,
        "dim_feedforward": 128,
        "feedforward_activation": "relu",
        "n_encoder_layers": 1,
        "n_decoder_layers": 1,
        "use_vae": True,
        "latent_dim": 8,
        "spiral_camera_embedding": True,
        "spiral_camera_order": [CAMERA],
    }
    tensors = {
        name: value.detach().cpu().to(torch.float32).contiguous()
        for name, value in policy.inference_state_dict().items()
    }
    tensors.update(
        {
            "normalize_inputs.buffer_observation_state.mean": torch.zeros(6),
            "normalize_inputs.buffer_observation_state.std": torch.ones(6),
            "normalize_inputs.buffer_follower_camera_wrist.mean": MEAN,
            "normalize_inputs.buffer_follower_camera_wrist.std": STD,
            "unnormalize_outputs.buffer_action.mean": torch.zeros(6),
            "unnormalize_outputs.buffer_action.std": torch.ones(6),
        }
    )
    (args.output / "config.json").write_text(json.dumps(config), encoding="utf-8")
    save_file(tensors, args.output / "model.safetensors")
    (args.output / "expected.json").write_text(
        json.dumps(
            {
                "state": STATE,
                "steps": 4,
                "action_dimensions": 6,
                "values": expected.flatten().tolist(),
            }
        ),
        encoding="utf-8",
    )
    print(args.output)


if __name__ == "__main__":
    main()
