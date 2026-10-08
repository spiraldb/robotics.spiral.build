"""The WARP-RM authors' model, sampler, labeler and loss, vendored from
https://github.com/uynitsuj/WARP-RM at 26f9894abdaf883af8db78a38e164764e5bf7c00 (MIT; see `LICENSE`),
the commit Score progress's scorer was verified against.

The pinned `warp-rm` package cannot be a dependency: importing `warp_rm.core` runs its `trainer` and
`run_logger`, which import `scripts.config`, a module the package does not ship, and `warp_rm.data`
imports OpenCV (beside LeRobot's headless OpenCV) and wants decord, which has no macOS arm64 wheel.
So the modules the progress job uses are copied here unchanged, in upstream's layout, each with a
header naming its source:

    models/aggregators/transformer.py   TransformerAggregator
    data/samplers.py                    ARSampler (and curve_parameterizations.py, which it imports)
    data/labelers.py                    RelativeCumulativeLabeler
    data/progress_curves.py             uniform_progress
    core/loss.py                        RMLoss (and ablation.py, which it imports)

The `__init__.py` files are ours: upstream's import the modules left out.
"""
