"""WARP-RM's sim recipe, the one the released `uynitsuj/warp-rm-sim-bottles-sss15` head was
trained with (its metadata: ablation `no_abs`, `standard_stride_src` 15, sampler `ar`): upstream's
`scripts/train.py --feature-stride 1 --source-standard-stride 15 --crop-mode squash`, every other
flag at its default, at 26f9894a. Each value names where upstream sets it. A run's steps, batch
size, learning rate, checkpoints, validation split and seed come from its request instead.

One deviation, for speed: upstream trains in fp32, while the job runs the head's forward pass and
loss under bf16 autocast on CUDA (`ProgressSession`), keeping fp32 weights, AdamW state and
checkpoints. Windows are drawn by upstream's sampler, but in worker processes (`ProgressWindows`).
"""

from __future__ import annotations

from .warp_rm_upstream.core.ablation import AblationConfig, get_ablation_configs
from .warp_rm_upstream.data.labelers import RelativeCumulativeLabeler
from .warp_rm_upstream.data.samplers import ARSampler, derive_ar_budget
from .warp_rm_upstream.models.aggregators.transformer import TransformerAggregator

# `Args.ablation`: the C51 head, temporal-diff tokens and stochastic depth; no absolute progress.
ABLATION = "no_abs"
# `Args.window_size`, and `config.MAX_SEQ_LEN`, the positional encoding's length.
WINDOW_SIZE = 32
MAX_SEQ_LEN = 32
# `--feature-stride 1`: one embedding per picture.
FEATURE_STRIDE = 1
# `--source-standard-stride 15`: a window at standard pace spans 31 x 15 pictures.
SOURCE_STANDARD_STRIDE = 15
# `ARSampler`'s frame rate, and the video's: Score progress scores 30 Hz video only.
FPS = 30
# The shortest interval a standard-pace window fits in, `(window_size - 1) * SSS + 2 *
# feature_stride` (`Args.min_frames`, docs/recipe.md). Shorter intervals still train: the sampler
# fits its path into them. It is also where Score progress's window plan shortens its stride.
STANDARD_WINDOW_FRAMES = (WINDOW_SIZE - 1) * SOURCE_STANDARD_STRIDE + 2 * FEATURE_STRIDE
# `Args.ar_*`: the fixed band the released results used (centre 1.5 s, half-range 1.0 s), AR(1)
# log-speed with alpha 0.5, Poisson(1.0) reversals, and a full-window flip half the time.
AR_CENTER_STRIDE_SEC = 1.5
AR_HALF_RANGE_SEC = 1.0
AR_ALPHA = 0.5
AR_LAMBDA_REVERSALS = 1.0
AR_P_FULL_FLIP = 0.5
# `DINOv3Backbone.DIM` and `config.N_HEADS`, `N_LAYERS`, `DROPOUT`.
EMBEDDING_SIZE = 768
HEADS = 8
LAYERS = 12
DROPOUT = 0.15
# The relative-progress C51 head's support (`run_experiment`, outside delta mode).
RELATIVE_BIN_RANGE = (-3.0, 3.0)
# `config.WEIGHT_DECAY`, `WARMUP_STEPS`, `GRAD_CLIP`.
WEIGHT_DECAY = 1.0e-3
WARMUP_STEPS = 1000
GRADIENT_CLIP = 1.0
# The frames the backbone sees: `config.IMAGE_SIZE`, resized by `--crop-mode squash`.
IMAGE_SIZE = 224
CROP_MODE = "squash"


class SimRecipe:
    """Upstream's pieces, built as `scripts/train.py`'s `run_experiment` and `build_model` build
    them for the sim recipe."""

    @staticmethod
    def ablation() -> AblationConfig:
        return get_ablation_configs()[ABLATION]

    @staticmethod
    def sampler() -> ARSampler:
        center_sec, half_range_sec, _, _ = derive_ar_budget(
            SOURCE_STANDARD_STRIDE, AR_CENTER_STRIDE_SEC, AR_HALF_RANGE_SEC
        )
        return ARSampler(
            window_size=WINDOW_SIZE,
            feature_stride=FEATURE_STRIDE,
            alpha=AR_ALPHA,
            center_stride_sec=center_sec,
            half_range_sec=half_range_sec,
            lambda_reversals=AR_LAMBDA_REVERSALS,
            p_full_flip=AR_P_FULL_FLIP,
            iid_speed=False,
        )

    @staticmethod
    def labeler() -> RelativeCumulativeLabeler:
        return RelativeCumulativeLabeler(
            feature_stride=FEATURE_STRIDE, source_standard_stride=SOURCE_STANDARD_STRIDE
        )

    @staticmethod
    def model() -> TransformerAggregator:
        """The progress head, from scratch: one camera, concatenated, bidirectional attention."""
        ablation = SimRecipe.ablation()
        low, high = RELATIVE_BIN_RANGE
        return TransformerAggregator(
            d_model=EMBEDDING_SIZE,
            n_heads=HEADS,
            n_layers=LAYERS,
            dropout=DROPOUT,
            max_seq_len=MAX_SEQ_LEN,
            backbone_dim=EMBEDDING_SIZE,
            use_temporal_diffs=ablation.use_temporal_diffs,
            stochastic_depth_p=(
                ablation.stochastic_depth_p if ablation.use_stochastic_depth else 0.0
            ),
            first_frame_pe_only=False,
            rel_bin_min=low,
            rel_bin_max=high,
            use_causal_attention=False,
            fusion="concat",
            n_cameras=1,
        )
