from __future__ import annotations

import asyncio
import gc
import json
import math

import httpx
import numpy as np
import pyarrow as pa

from spiral_training_runtime.arrow import decode_training_batch
from spiral_training_runtime.config import TrainingConfig
from spiral_training_runtime.service import (
    ARROW_STREAM_CONTENT_TYPE,
    POLICY_ARTIFACT_CONTENT_TYPE,
    create_app,
)


def training_request() -> dict:
    return {
        "model": {
            "policy": "act",
            "state_dimensions": 2,
            "action_dimensions": 2,
            "camera_streams": ["observation.images.wrist"],
            "image_width": 32,
            "image_height": 32,
            "chunk_size": 2,
            "hidden_dimensions": 16,
            "latent_dimensions": 4,
            "attention_heads": 4,
            "feedforward_dimensions": 32,
            "encoder_layers": 1,
            "vae_encoder_layers": 1,
            "decoder_layers": 1,
            "kl_weight": 1.0,
            "learning_rate": 1.0e-5,
            "total_steps": 2,
            "seed": 7,
        },
        "device": "cpu",
        "initialization": "random",
    }


def policy_config() -> dict:
    return {
        "type": "act",
        "n_obs_steps": 1,
        "normalization_mapping": {
            "VISUAL": "MEAN_STD",
            "STATE": "MEAN_STD",
            "ACTION": "MEAN_STD",
        },
        "input_features": {
            "observation.images.wrist": {"type": "VISUAL", "shape": [3, 32, 32]},
            "observation.state": {"type": "STATE", "shape": [2]},
        },
        "output_features": {"action": {"type": "ACTION", "shape": [2]}},
        "chunk_size": 2,
        "n_action_steps": 2,
        "vision_backbone": "resnet18",
        "replace_final_stride_with_dilation": False,
        "pre_norm": False,
        "dim_model": 16,
        "n_heads": 4,
        "dim_feedforward": 32,
        "feedforward_activation": "relu",
        "n_encoder_layers": 1,
        "n_vae_encoder_layers": 1,
        "n_decoder_layers": 1,
        "use_vae": True,
        "latent_dim": 4,
        "spiral_camera_embedding": False,
        "spiral_camera_order": ["observation.images.wrist"],
    }


def fixed_list(values: np.ndarray, rows: int) -> pa.FixedSizeListArray:
    flat = pa.array(values.reshape(-1))
    return pa.FixedSizeListArray.from_arrays(flat, len(values.reshape(rows, -1)[0]))


def arrow_stream(columns: dict[str, pa.Array]) -> bytes:
    batch = pa.RecordBatch.from_arrays(list(columns.values()), names=list(columns))
    sink = pa.BufferOutputStream()
    with pa.ipc.new_stream(sink, batch.schema) as writer:
        writer.write_batch(batch)
    return sink.getvalue().to_pybytes()


def training_body() -> bytes:
    return arrow_stream(
        {
            "states": fixed_list(np.zeros((1, 2), dtype=np.float32), 1),
            "images": fixed_list(np.zeros((1, 3 * 32 * 32), dtype=np.float32), 1),
            "actions": fixed_list(np.zeros((1, 4), dtype=np.float32), 1),
            "action_is_pad": fixed_list(np.zeros((1, 2), dtype=np.uint8), 1),
        }
    )


def inference_body() -> bytes:
    return arrow_stream(
        {
            "state": fixed_list(np.zeros((1, 2), dtype=np.float32), 1),
            "image_0": fixed_list(np.zeros((1, 3 * 32 * 32), dtype=np.uint8), 1),
        }
    )


def test_arrow_values_are_zero_copy_numpy_views() -> None:
    config = TrainingConfig.from_request(training_request())
    batch = decode_training_batch(training_body(), config)
    assert batch.states.shape == (1, 2)
    assert batch.images.shape == (1, 3 * 32 * 32)
    assert not batch.states.flags["OWNDATA"]
    assert not batch.images.flags["OWNDATA"]


def test_http_surface_reports_pytorch_without_constructing_a_model() -> None:
    async def exercise() -> None:
        transport = httpx.ASGITransport(app=create_app())
        async with httpx.AsyncClient(transport=transport, base_url="http://runtime") as client:
            response = await client.get("/v1/health")
            assert response.status_code == 200
            assert response.json()["id"] == "pytorch.lerobot"

            response = await client.post(
                "/v1/sessions/missing/train?step=1",
                content=training_body(),
                headers={"content-type": ARROW_STREAM_CONTENT_TYPE},
            )
            assert response.status_code == 404
            assert "does not exist" in response.json()["error"]

    asyncio.run(exercise())


def test_real_pytorch_act_train_checkpoint_and_infer_round_trip() -> None:
    async def exercise() -> None:
        transport = httpx.ASGITransport(app=create_app(policy_device="cpu"))
        async with httpx.AsyncClient(transport=transport, base_url="http://runtime") as client:
            created = await client.post("/v1/sessions", json=training_request())
            assert created.status_code == 201, created.text
            session_id = created.json()["session_id"]
            assert created.json()["info"]["backend"]["id"] == "pytorch.lerobot"

            trained = await client.post(
                f"/v1/sessions/{session_id}/train?step=1",
                content=training_body(),
                headers={"content-type": ARROW_STREAM_CONTENT_TYPE},
            )
            assert trained.status_code == 200, trained.text
            assert math.isfinite(trained.json()["loss"]["total"])

            checkpoint_response = await client.post(
                f"/v1/sessions/{session_id}/checkpoint",
                json={
                    "manifest_json": "{}",
                    "state_mean": [0.0, 0.0],
                    "state_std": [1.0, 1.0],
                    "action_mean": [0.0, 0.0],
                    "action_std": [1.0, 1.0],
                },
            )
            assert checkpoint_response.status_code == 200, checkpoint_response.text
            assert checkpoint_response.headers["x-spiral-checkpoint-format"] == "safetensors"
            checkpoint = checkpoint_response.content
            assert len(checkpoint) > 1_000

            deleted = await client.delete(f"/v1/sessions/{session_id}")
            assert deleted.status_code == 204
            gc.collect()

            config_bytes = json.dumps(policy_config(), separators=(",", ":")).encode()
            artifact = len(config_bytes).to_bytes(8, "little") + config_bytes + checkpoint
            policy_created = await client.post(
                "/v1/policies",
                content=artifact,
                headers={"content-type": POLICY_ARTIFACT_CONTENT_TYPE},
            )
            assert policy_created.status_code == 201, policy_created.text
            policy_id = policy_created.json()["session_id"]
            assert policy_created.json()["specification"]["kind"] == "act"

            inferred = await client.post(
                f"/v1/policies/{policy_id}/infer",
                content=inference_body(),
                headers={"content-type": ARROW_STREAM_CONTENT_TYPE},
            )
            assert inferred.status_code == 200, inferred.text
            result = inferred.json()
            assert result["steps"] == 2
            assert result["action_dimensions"] == 2
            assert len(result["values"]) == 4
            assert all(math.isfinite(value) for value in result["values"])

    asyncio.run(exercise())
