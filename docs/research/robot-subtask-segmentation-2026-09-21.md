# Automatic subtask timelines for robot episodes

Research checked September 21, 2026. Focus: turning an existing, untrimmed robot episode into time intervals with meaningful action descriptions. This is a literature and implementation review; no models were run on Spiral recordings.

The capability is practical for proposing annotations, but the public evidence does not support treating arbitrary model-generated timelines as ground truth. For Spiral, my first experiment would compare Macrodata Refiner and LeRobot's annotation pipeline on our own episodes. Berkeley's RoboSegNet and InternRobotics' Cortex are the most relevant additional directions when precise boundaries and robot telemetry matter. The strongest full-benchmark commercial result I found is FPV Labs' Vidur; its evaluation is vendor-reported.

**What problem are we solving?**

The relevant terms are temporal action segmentation, subtask discovery, and dense video captioning. They describe related but different outputs:

| Setting | Model receives | Model must produce | Relevance |
| --- | --- | --- | --- |
| Open-ended annotation | Episode, optionally its overall instruction | Both boundaries and previously unspecified descriptions | Closest to “automatically show what the robot is doing” |
| Prescribed-stage alignment | Episode plus a list or template of subtasks | Where those known stages occur | Useful for repeated folding demonstrations |
| Online stage recognition | Observations available so far | Current stage or whether to switch stages | Needed only if “at this moment” means during live execution |

I interpret the immediate Spiral use case as offline annotation displayed while scrubbing a recorded episode. That permits future-frame context. A live model cannot use that same evidence without introducing delay.

An illustrative folding timeline could be: grasp the near corners → lift the near edge → bring that edge onto the far edge → release → smooth the fold → grasp the side edge → make the second fold. This is an example of desired granularity, not a prescribed sequence or an annotation of an actual recording. The model must preserve the actions that happened, including retries and incomplete folds.

**The most comparable public results**

[WGO-Bench](https://huggingface.co/datasets/macrodata/WGO-Bench) has 100 episodes and 743 annotated segments from human egocentric, external robot-camera, and robot head-camera footage. It is unusually close to this product requirement.

The benchmark's segment score requires temporal intersection-over-union of at least 0.75. Semantic end-to-end F1 additionally requires the label to pass an LLM judge. Macrodata snaps the first and last predicted episode boundaries to the reference before scoring. These are strict interval-and-description metrics, not percentages of correctly labeled frames. [Scoring protocol](https://macrodata.co/blog/annotating-robot-video-subtasks).

| System/configuration | Segment F1 | Semantic end-to-end F1 | Evidence and availability |
| --- | ---: | ---: | --- |
| FPV Labs Vidur | ~0.49, rounded in chart | **0.386** | Vendor evaluation; proprietary early access. [Report](https://www.fpvlabs.ai/essays/vidur-action-label) |
| Perceptron Egocentric, with instruction | 0.370 | **0.280** | Vendor evaluation; proprietary early access. [Report](https://www.perceptron.inc/blog/introducing-perceptron-egocentric-api) |
| Macrodata, Gemini 3.5 Flash with seeded relabeling | 0.302 | **0.168** | Public recipe and open pipeline; hosted model. [Report](https://macrodata.co/blog/annotating-robot-video-subtasks) |
| LeRobot annotation pipeline, Qwen3.6-27B | 0.197 | **0.121** | Open pipeline and self-hostable model; released annotated outputs. [Report](https://lerobot-robots-that-talk.hf.space/) |

These are reported results on the full benchmark, not my independent rerun or a neutral leaderboard. Implementations, input representations, and inference budgets differ. Vidur explicitly frames its comparison as a comparison of pipelines. Neither commercial score establishes performance on our cloth-folding recordings.

Macrodata's separate segmentation sweep reached 0.306; 0.302 is the value paired with its final end-to-end experiment. Its 61.0% labeling result uses supplied human boundaries, while 78.1% is conditional on correctly matched predicted intervals. These are different denominators. [Experiment details](https://macrodata.co/blog/annotating-robot-video-subtasks).

The LeRobot article's claimed “1.8×” improvement is inconsistent with the cited end-to-end values 0.121 and 0.110. The table above retains its raw scores without repeating that multiplier. [LeRobot evaluation](https://lerobot-robots-that-talk.hf.space/).

**The practical open options**

**Macrodata Refiner is the clearest baseline for the exact feature.** Its `subtask_annotation` operation returns intervals with `start_sec`, `end_sec`, and `subtask`. It shows a VLM timestamped image grids, by default sampling every 0.5 seconds. Optional `subtask_labeling` verifies and revises each initial description using the previous, current, and next intervals. This needs no per-episode hand-written subtask list. [Operation documentation](https://macrodata.co/docs/episode-operations/subtask-annotation).

The framework is [open source](https://github.com/macrodata-labs/refiner), and its [LeRobot example](https://macrodata.co/docs/examples/annotations/subtask-annotations) covers reading episodes, annotating, and writing the result. Open pipeline does not mean open model weights: the documented example uses Google's hosted Gemini model. My assessment: strongest starting point for testing the exact output contract, with relatively little custom model work.

**Hugging Face LeRobot is the strongest ecosystem fit when self-hosting matters.** The documented `lerobot-annotate` path can populate subtasks and other language annotations directly in a LeRobot dataset. It describes the visible episode first, then derives consecutive subtasks; seeded relabeling is optional. The documentation inspected is the **main branch**, explicitly requiring installation from source, so this should not be assumed available in every stable package version. [Annotation pipeline](https://huggingface.co/docs/lerobot/main/en/annotation_pipeline).

Its format supports a persistent active subtask that resolves at a given frame, closely matching the requested timeline behavior. [Language columns](https://huggingface.co/docs/lerobot/main/en/language_and_recipes). A practical parameter to inspect is the default `plan_max_steps=8`: long, fine-grained episodes may need a larger limit. This is a documented configuration choice, not evidence that eight subtasks is a good universal granularity. [Configuration](https://huggingface.co/docs/lerobot/main/en/annotation_pipeline).

**InternRobotics Cortex is particularly relevant to repeated tasks.** Its released annotation workflow uses a VLM for initial labels, lets a person correct several seed episodes, then transfers the corrected templates to other episodes using camera features, state/action data, and timing priors. Dynamic programming fits the boundaries; nearby low-motion frames help refine them. Different seed templates can have different numbers of subtasks. [Annotation guide](https://github.com/InternRobotics/Cortex/blob/main/docs/annotation.md).

The broader July 2026 research standardizes subtasks into 32 skill primitives and reports annotating over 4,000 hours. Its policy/planning evaluations should not be mistaken for an open-vocabulary segmentation benchmark. [Cortex paper](https://arxiv.org/abs/2607.05377). My assessment: promising for a collection of similar folding demonstrations after a few careful corrections, with generalization to unusual recoveries still needing evaluation.

**LeRobot SARM offers a simpler prescribed-stage route.** Its annotation tooling accepts sparse and dense subtask names and uses a VLM to locate them. This is useful when we already know the vocabulary we want, such as first fold, second fold, and final placement. It is different from discovering unknown task structure. The no-annotation `single_stage` mode treats the episode as one stage and does not solve this request. [SARM documentation](https://huggingface.co/docs/lerobot/sarm).

**The most relevant specialized research**

**Berkeley RoboSegNet — visual and kinematic boundary detection.** This CVPR 2026 Findings work combines camera features with robot motion features and predicts transition times. Its RoboSegData benchmark contains 11,210 trajectories across 21 tasks, including three held-out tasks. The reported segment F1 at 0.50 IoU is 74.62% in the multi-task setting and 74.83% on held-out tasks. These numbers measure segmentation on its own dataset, not correctly generated natural-language labels, and cannot be ranked against WGO's end-to-end scores. [Project and results](https://berkeleyautomation.github.io/RoboSegNet/).

This is the paper I would prioritize for improving timing after an initial VLM experiment. Its [public repository](https://github.com/KeplerC/RoboSegNet) includes feature extraction, training, evaluation, and inference components. I did not verify an immediately downloadable trained checkpoint; the README retains placeholder installation/citation text. Treat integration effort as unresolved. A language-labeling stage would still be needed.

**EgoANT — open-weight, coarse-to-fine annotation.** Its report uses Qwen models to propose episode-wide boundaries and then revisit local regions. Best reported end-to-end F1 is 0.1542, but only on WGO's 25-video HomER subset, so it does not belong in the full-benchmark ranking. It also finds that neighboring-frame context can contaminate captions with adjacent actions; recipes are model-dependent. [Author report](https://www.shuai-zhou.com/egoant-report/).

**RoboSubtaskNet — supervised recognition with a fixed vocabulary.** This February 2026 work trains a temporal network on labeled manipulation videos. It is relevant when a team can supply training examples and wants consistent domain-specific labels. Its strong results on its own RoboSubtask dataset are not evidence of automatic discovery on arbitrary new robot tasks. [Paper](https://arxiv.org/abs/2602.10015).

**StageGuard — a new online direction.** Submitted September 17, 2026, it distills VLM reasoning into a smaller model that decides when a skill is complete and a hierarchical controller should switch. This addresses live stage transitions; it does not demonstrate arbitrary offline episode segmentation and captioning. [Preprint](https://arxiv.org/abs/2609.20791).

Two supporting results help explain where the field is going. ATLAS is a human annotation tool whose assembly study found substantially better boundary placement when annotators could see synchronized robot time-series signals. It supports evaluating telemetry alongside video, but is not an automatic segmenter. [ATLAS](https://arxiv.org/abs/2604.26637). NVIDIA/UC San Diego's DeMiAn reports policy gains from densely relabeling existing demonstration clips; this supports the value of richer language, not the correctness of any particular boundary detector. [DeMiAn](https://arxiv.org/abs/2605.17077).

**How I would apply this to Spiral**

My recommendation is an experiment with automatic interval proposals, retaining the original episode and attaching a subtask track. At the playhead, Spiral would resolve and display the active interval. This avoids making physical clip extraction a prerequisite for understanding an episode.

Start with one clear annotation convention. For folding, distinguish a meaningful change to the cloth from a small correction within that action. Decide whether “grasp, lift, fold, release” should be separate labels or whether a whole fold is the intended unit. Both can be useful, but evaluating one against references written for the other produces misleading errors.

Compare Refiner and LeRobot on the same small set of recordings before choosing a model. If either yields useful descriptions but poor timing, test Cortex for repeated-task alignment or investigate RoboSegNet for learned multimodal boundaries. This ordering is an engineering recommendation, not a claim that combining these methods has already achieved a published result.

I would make a 20–30-episode pilot deliberately include straightforward folds, occluded grasps, pauses, regrasping, retries, and incomplete attempts. Have two people independently mark a small overlapping subset first: if they disagree systematically, refine the convention before interpreting model scores. Keep the eventual evaluation episodes separate from examples used to tune prompts or templates.

Measure these outcomes separately:

- Boundary error in seconds and segment F1 at both 0.50 and 0.75 temporal IoU.
- Whether the action, manipulated cloth region, direction, and resulting state are described correctly.
- Whether short actions and retries disappear or get merged incorrectly.
- Human correction time per episode, compared with annotating from scratch.
- Annotation compute cost and elapsed processing time.

For an exploratory timeline, correction time and navigational usefulness may matter more than strict benchmark F1. For training labels, evaluate the resulting policy as a separate experiment: agreement with a human segmentation convention does not establish a policy-learning gain.

No study reviewed here establishes a universal best model for cloth folding. The actionable conclusion is to use an existing open annotator for the first Spiral experiment, test granularity on our footage, and reserve specialized boundary modeling for the errors that experiment actually exposes.
