**Robot-learning data curation: research, industry practice, and directions beyond WARP**

Study date: September 21, 2026. Scope: selecting, scoring, weighting, segmenting, and improving data used to train robot policies, with manipulation as the main application. This is a focused literature and public-artifact review, not an exhaustive systematic review or an independent reproduction of the experiments.

**Main finding.** WARP belongs to a substantial research area with several different objectives. Some methods estimate whether an action makes progress; others measure redundancy, retrieve relevant behavior, optimize dataset mixtures, or estimate whether a demonstration helps a particular policy. My assessment is that Spiral should support these distinct decisions without presenting their scores as interchangeable measures of “quality.”

I started from our demo and WARP's references, followed related papers and author repositories, and searched for newer work through the study date. The comparisons below use papers, author project pages, released repositories/model cards, and companies' own technical documentation. Numerical results are author-reported and specific to their evaluation settings. A public research affiliation does not establish deployment in that organization's production stack.

**What our existing demo establishes.** The current implementation uses a pinned, task-specific simulated-bottle checkpoint. It averages velocity over 30-frame chunks and uses a zero minimum-velocity policy. It is deliberately restricted to the supported dataset and original recordings. These are properties of our implementation, visible in [progress.rs](/Users/xiangpeng/coding/spiral-robotics/crates/common/src/curation/progress.rs:1), and the [recording script](/Users/xiangpeng/coding/spiral-robotics/docs/curation-demo-recording-script.md:1) identifies the footage as simulation. The demo establishes scoring, review, and selection behavior; it does not establish a training improvement for that particular recipe.

The WARP paper's main training recipe instead gates on terminal-frame velocity above a reference-pace threshold, with weighting that differs between hardware and simulation experiments. Its strongest controlled simulation comparison uses 512 paired scenes and matched 31.5% retention across curation methods: 290 bottles/hour for WARP versus 271 for DemInf, 270 for SCIZOR, and 237 for full-data BC. Those figures make a more useful reference than the exceptional “18×” folding result, which arose in a setting where vanilla BC performed very poorly. The authors explicitly limit the evidence to two real tasks and one simulated task on one bimanual embodiment; preserving useful recovery behavior remains unestablished. Our recipe therefore should not inherit the paper's numerical claims. [WARP v4, methods, experiments, and limitations](https://arxiv.org/html/2606.28320v4)

**The questions researchers are separating**

| Decision | Unit being judged | Representative work | What the answer does not establish |
| --- | --- | --- | --- |
| Is this recording usable? | Sensor stream, frame, interval, episode | [Scale's QC study](https://labs.scale.com/blog/from-video-review-to-measurement) | That imitating the behavior improves a policy |
| Does this behavior advance the task? | Frame, transition, action chunk | [WARP](https://uynitsuj.github.io/warp-rm/), [SARM](https://qianzhong-chen.github.io/sarm.github.io/) | Final success or universal usefulness |
| Are we repeating the same training signal? | State-action segment | [SCIZOR](https://ut-austin-rpl.github.io/SCIZOR/) | That a rare segment is correct |
| Which data helps this target policy/task? | Episode or subtrajectory | [CUPID](https://cupid-curation.github.io/), [DataMIL](https://robin-lab.cs.utexas.edu/datamodels4imitation/) | A permanent, model-independent quality grade |
| How should sources be sampled? | Dataset/domain | [Re-Mix](https://arxiv.org/abs/2408.14037) | Which exact moment is defective |
| What experience should enter the next learning round? | Autonomous rollout, correction, generated demonstration | [RECAP](https://www.physicalintelligence.company/download/pistar06.pdf), [AutoRT](https://deepmind.google/blog/shaping-the-future-of-advanced-robotics/) | That collection alone produces improvement |

**The closest and most useful research**

**1. SCIZOR — add redundancy removal to progress filtering.** UT Austin and NVIDIA; ICRA 2026, initially posted in 2025. SCIZOR combines a self-supervised progress estimator with deduplication in joint visual-action representations. The second component is the most useful extension to our present demonstration: two perfectly successful segments can still provide nearly identical training information. Its evaluations include RoboMimic, an Open-X mixture, and real robot data. [Project and results](https://ut-austin-rpl.github.io/SCIZOR/)

The paper acknowledges empirically chosen deletion thresholds and a progression assumption that can mishandle repeated stirring or necessary waiting. Thus, “remove repetition” needs task context and downstream evaluation. The released repository includes training and scoring paths, rather than establishing a universal pretrained curation service. [Paper limitations](https://arxiv.org/html/2505.22626v1), [code](https://github.com/UT-Austin-RPL/SCIZOR)

**2. SARM and SARM2 — explain progress using task structure.** Stanford, Berkeley, and XDOF, with SJTU also participating in SARM2. SARM jointly predicts task stage and within-stage progress from subtask annotations, then uses reward-aligned behavior cloning. It is attractive when users need an explanation such as “flattening completed; folding stalled.” SARM was accepted at ICLR 2026. [SARM project](https://qianzhong-chen.github.io/sarm.github.io/)

SARM2, posted in June 2026, uses a shared action-primitive vocabulary and multi-task value modeling, with three cameras and proprioception. Its learning loop includes a one-time annotation of approximately 100 policy rollouts to adapt the reward model before autonomous iterations. This matters: multi-task capability does not remove all annotation or adaptation requirements. The authors name their loop **SPIRAL**, an unrelated acronym for Self-Policy Improvement via Reward-Aligned Learning. [SARM2 methods and workflow](https://qianzhong-chen.github.io/sarm2.github.io/)

Training and offline labeling code is public in [OpenSARM](https://github.com/xdofai/opensarm); LeRobot also documents [SARM training and RA-BC](https://huggingface.co/docs/lerobot/sarm). I verified these implementations, not a ready-made checkpoint for every proposed customer task.

**3. Robometer — the strongest first candidate for broader task-conditioned scoring.** The collaboration spans USC, Washington, UT Dallas, MIT, Ai2, and NVIDIA; RSS 2026. Robometer combines progress supervision with comparisons between trajectories, including failures. It outputs progress, success, and trajectory preferences. The authors demonstrate failure detection, reward learning, and retrieval followed by policy training. In two retrieval experiments, policies trained on 100 selected segments achieved 13/20 and 14/20 successes. [Project and experiments](https://robometer.github.io/)

It works on subsampled video and cannot directly observe hidden contact forces; rare failures and long histories remain limitations. [Paper](https://arxiv.org/html/2603.02115v2) Public [4B weights](https://huggingface.co/robometer/Robometer-4B) and [inference/training code](https://github.com/robometer/robometer) make it practical to evaluate. The project labels the complete RBM-1M data link “Coming Soon,” while the repository provides data-processing/download paths; I would verify packaging before promising full training reproduction.

**4. RoboReward — useful episode-level outcome judgment.** Lee and colleagues introduce a reward benchmark and 4B/8B models using real-robot corpora, with counterfactual task relabeling and temporal clipping to create negative and partial-success examples. [Paper](https://arxiv.org/abs/2601.00675)

The released model's interface is a task instruction plus video, producing a discrete final-state score from 1 to 5. This is suitable for ranking completion or reviewing task-label mismatches, but it should not be described as a drop-in replacement for WARP's signed local velocity. Public model cards document [4B](https://huggingface.co/teetone/RoboReward-4B) and [8B](https://huggingface.co/teetone/RoboReward-8B) usage. My proposed role is an outcome signal alongside a local progress signal, with disagreements sent to review.

**5. TOPReward — use an existing VLM without reward-model training.** Washington, Ai2, Amazon, and UNC collaborators; February 2026. It derives task progress from token probabilities rather than asking a VLM to generate a numeric completion percentage. The project evaluates reward ordering, success detection, and reward-aligned BC across multiple robot platforms. [Project](https://topreward.github.io/webpage/)

This is a useful inexpensive-to-start experimental baseline in terms of training work, although inference is still VLM inference and suitable logits access is required. High correlation with temporal order does not by itself prove accurate mistake localization or improved policy success. [Paper](https://arxiv.org/abs/2602.19313), [code](https://github.com/TOPReward/TOPReward)

**6. DemInf — assess episode informativeness from states and actions.** Google DeepMind Robotics and Stanford; RSS 2025. It estimates state-action mutual information in learned latent spaces and aggregates contributions into demonstration scores. Its distinctive attraction is selection without human quality labels or online policy evaluation. The paper includes simulation, ALOHA, and Franka experiments. [Paper](https://arxiv.org/html/2502.08623v1), [author project](https://joeyhejna.com/demonstration-info/)

Its limitations are revealing: predictable pauses can score well; outlier trajectories can be favored; and full-data training slightly outperformed filtering on one harder Franka task. This makes it a complementary episode-level baseline, not an automatic ground truth for quality. My expectation is that combining an information criterion with a validated progress signal is worth testing, but gains from that combination must be measured.

**7. CUPID — attribute robot performance back to demonstrations.** Stanford, Cambridge, NVIDIA Research, and Toyota Research Institute; CoRL 2025. CUPID uses a trained policy and scored evaluation rollouts to estimate how training demonstrations influence expected return. It can filter an existing dataset or select useful new demonstrations. Hardware cases include brittle strategies and spurious correlations, not just visibly poor motion. [Project](https://cupid-curation.github.io/)

This is a substantial conceptual step beyond a generic quality detector: usefulness depends on policy behavior. Its influence estimates are approximations, and the paper reports attribution cost comparable to policy training on the full dataset; few rollouts also increase estimator variance. [Paper limitations](https://arxiv.org/html/2506.19121v2), [code](https://github.com/agiachris/cupid)

**8. DataMIL and Quality over Quantity — estimate usefulness from a target validation set.** DataMIL, from UT Austin, MIT, and Stanford, uses datamodels and target-demonstration validation loss as a practical surrogate for expensive robot rollouts. It groups samples temporally, selects data, and co-trains with target demonstrations; reported experiments cover more than 60 simulation and real tasks. [Project](https://robin-lab.cs.utexas.edu/datamodels4imitation/), [code](https://github.com/UT-Austin-RobIn/datamil)

Quality over Quantity, from KAIST with Berkeley and NYU collaborators, was posted in March 2026. It uses normalized gradient-based influence approximations, maximum relevance across validation samples, and trajectory aggregation. Its evidence includes RoboMimic and real robots. [Paper](https://arxiv.org/html/2603.09056v1)

My interpretation: both are useful for “find data that teaches this skill,” but validation loss is a proxy for actual robot performance. The target examples define what counts as useful, so a narrow validation set can steer selection away from other valuable behavior.

**9. Re-Mix — curate the sampling recipe across datasets.** Stanford-led work; CoRL 2024. Re-Mix learns domain weights using robust optimization and reference-policy losses. It evaluates mixtures from Bridge and Open-X on WidowX and Franka. This addresses a question WARP does not: how frequently should different sources appear during training? [Paper](https://arxiv.org/html/2408.14037v1)

The paper reports useful 25% subsets, but computing new weights requires two full-data policy-training runs, and abnormal action distributions can distort weights. For Spiral, this suggests keeping dataset membership separate from sampling weight. It is most relevant once a user trains over several sources or domains. The authors release [code](https://github.com/jhejna/remix).

**10. STRAP — retrieve reusable behavior inside longer demonstrations.** ICLR 2025. STRAP uses visual representations and subsequence dynamic time warping to find relevant subtrajectories from a large corpus using a few target demonstrations. It demonstrates policy learning in simulation and hardware. [Conference paper](https://proceedings.iclr.cc/paper_files/paper/2025/hash/a06e129e01e0d2ef853e9ff67b911360-Abstract-Conference.html), [code](https://github.com/WEIRDLabUW/STRAP)

The interesting product interaction is “find more examples of this maneuver,” including maneuvers embedded in differently named tasks. Retrieval quality and behavioral quality are separate: a matching motion can still be unsuccessful. My recommendation is to preserve sequence and action context when testing retrieval, rather than relying on visually similar individual frames.

**11. Demo-SCORE — let robot experience reveal which strategies are reproducible.** Stanford; RSS 2025. It trains a success/failure classifier from policy rollouts and uses it to filter demonstrations. The authors report 15–35 percentage-point success gains across five real-world ALOHA tasks. Their examples show why a strategy natural for a human teleoperator may be unreliable for the learned robot. [Project](https://anniesch.github.io/demo-score/)

The central dependency is online experience representative of the demonstration strategies. The authors acknowledge that a policy unable to reproduce some strategies may provide misleading evidence about them. This is a good conceptual baseline for a future feedback loop linking failures, source data, and retraining. [Paper](https://arxiv.org/abs/2503.03707)

**Industry labs are pursuing several different parts of the problem**

| Organization or collaboration | Publicly demonstrated direction | Implication for this study |
| --- | --- | --- |
| XDOF, Berkeley, Stanford | [WARP](https://uynitsuj.github.io/warp-rm/) and [SARM2](https://qianzhong-chen.github.io/sarm2.github.io/) | Closest lineage to our demonstration: dense scoring connected to training |
| NVIDIA and UT Austin | [SCIZOR](https://ut-austin-rpl.github.io/SCIZOR/) | Progress filtering and redundancy are distinct selection problems |
| Google DeepMind and Stanford | [DemInf](https://www.roboticsproceedings.org/rss21/p023.pdf) | Episode quality can use actions as well as appearance |
| Toyota Research Institute with Stanford, Cambridge, NVIDIA | [CUPID](https://cupid-curation.github.io/) | Training-data decisions can be conditioned on observed policy performance |
| Physical Intelligence | [RECAP / π*0.6](https://www.physicalintelligence.company/download/pistar06.pdf) | Mix demonstrations, autonomous experience, and expert corrections using value estimates and advantage conditioning |
| Scale Labs | [Robot and egocentric QC](https://labs.scale.com/blog/from-video-review-to-measurement) | Recording validity and review efficiency are separate from downstream policy selection |
| BAAI and Peking University collaborators | [Robo-Dopamine](https://robo-dopamine.github.io/) | Multi-view and step-aware reward modeling for precise manipulation |
| AgiBot, OpenDriveLab-associated collaborators | [Data-diversity and velocity-debiasing study](https://arxiv.org/html/2507.06219v1) | Task coverage and operator-speed variation should not be collapsed into one diversity score |

**Scale provides an especially useful industrial counterpoint.** Its July 2026 study found that broad VLM pass/reject judgments were hard to tune. Focused checks with explicit evidence worked better for specific camera defects. In a separate egocentric-video track, a 120-feature LightGBM model achieved reported out-of-time AUC 0.8003; stronger image encoders did not improve that setup. The authors position these systems as human-review triage rather than autonomous rejection. This is evidence about QC on their data, not proof of better trained policies. [Scale technical study](https://labs.scale.com/blog/from-video-review-to-measurement)

**Physical Intelligence's work moves curation into the learning loop.** RECAP combines demonstrations, autonomous rollouts, and teleoperated interventions; value estimates support advantage-conditioned policy improvement. It is a training system requiring deployment experience and feedback, not a standalone quality model that can simply be attached to arbitrary episodes. [Technical paper](https://www.physicalintelligence.company/download/pistar06.pdf)

**Collection and generation are adjacent, important directions.** DeepMind's AutoRT uses foundation models to propose and select tasks and collect diverse real robot experience. [AutoRT](https://deepmind.google/blog/shaping-the-future-of-advanced-robotics/) NVIDIA's GR00T-Mimic expands seed demonstrations into synthetic trajectories, followed by evaluation and refinement. [NVIDIA workflow](https://developer.nvidia.com/blog/?p=94583) My inference is that curation needs to preserve collection and generation provenance if these sources are combined; synthetic volume is not itself evidence of real-world coverage.

**Newer work worth watching, with more limited conclusions**

- **ATHENA, June 2026:** efficient influence estimation for billion-parameter, multi-task VLAs. Its reported experiments retain 50% of simulation demonstrations or 66.7% across six real tasks while matching or improving full-data fine-tuning. The advertised 313.4× figure concerns influence computation, not end-to-end training acceleration. [Preprint](https://arxiv.org/abs/2606.16208)
- **Robo-Dopamine 2.0, August 2026:** incorporates history and an explicit signed space for progress, failure, and recovery. This directly addresses ambiguity in simple progress curves. The official repository documents a preview checkpoint; it should be evaluated as a recent release. [Preprint](https://arxiv.org/abs/2608.15680), [repository](https://github.com/FlagOpen/Robo-Dopamine)
- **AnyviewMeter, September 17, 2026:** adapts Robometer with camera geometry and synchronized multi-view attention. Its reported improvements concern reward prediction under camera changes; they do not establish corresponding policy-training gains. Particularly relevant to our multi-camera episodes. [Preprint](https://arxiv.org/abs/2609.20106)
- **ROSER, March 2026, ICLR DATA-FM workshop:** few-shot retrieval of task segments from long logs. The abstract emphasizes retrieval accuracy and efficiency; I would require downstream policy evidence before treating it as proven training improvement. [Paper](https://arxiv.org/abs/2603.01474)
- **Learning While Deploying, May 2026:** a shared experience loop across 16 dual-arm robots and eight tasks, using autonomous rollouts and interventions. It illustrates the data-management requirements of continual post-training, rather than a replacement for a curation scorer. [Preprint](https://arxiv.org/abs/2605.00416)
- **ReWiND, CoRL 2025:** USC, Amazon Robotics, and KAIST collaborators use language-conditioned rewards and reversed video to support adaptation to new task variations. It is useful background for WARP's augmentation lineage and reward relabeling. [Project](https://rewind-reward.github.io/)

**What can be tried, and what needs training integration**

The burden judgments here are my assessment of the documented inputs and workflow, not measured runtimes. “Public” means the linked artifact was accessible, not that I installed or reproduced it.

| Candidate | Required evidence beyond raw episode video | Public starting point | Practical role |
| --- | --- | --- | --- |
| Robometer | Task instruction | [Weights and model card](https://huggingface.co/robometer/Robometer-4B) | First broader scoring experiment |
| RoboReward | Task instruction | [8B model card](https://huggingface.co/teetone/RoboReward-8B) | Episode completion comparison |
| TOPReward | Task text and compatible VLM probabilities | [Code](https://github.com/TOPReward/TOPReward) | Training-free reward baseline |
| SARM/SARM2 | Appropriate annotations, training data; model-specific views/state | [OpenSARM](https://github.com/xdofai/opensarm) | More structured progress explanation |
| SCIZOR | Actions, representations, a trained progress estimator | [Code](https://github.com/UT-Austin-RPL/SCIZOR) | Redundancy and transition selection |
| STRAP | Reference demonstrations and a corpus | [Code](https://github.com/WEIRDLabUW/STRAP) | Task-specific segment retrieval |
| DataMIL | Target demonstrations and policy-training access | [Code](https://github.com/UT-Austin-RobIn/datamil) | Target-aware subset selection |
| CUPID | Trained policy, gradients, scored rollouts | [Code](https://github.com/agiachris/cupid) | Selection tied to actual policy behavior |
| Re-Mix | Multiple domains and policy training | [Code](https://github.com/jhejna/remix) | Learned sampling proportions |

**The infrastructure market already includes reward scoring.** Daft documents a Robometer pipeline that decodes episode video, produces progress/success columns, and filters with queries. Macrodata's Refiner documents a similar operation with frame indices and timestamps. These are implementation examples, not independent proof that their selected data improves robot performance. [Daft documentation](https://docs.daft.ai/en/stable/examples/reward-scoring-physical-ai/), [Macrodata documentation](https://macrodata.co/docs/episode-operations/reward-scoring)

My product inference is that integrating another open model alone offers limited differentiation. Spiral can be more useful if a user can inspect the original synchronized evidence, understand exactly which training samples changed, reproduce the recipe, and connect that change to an evaluation. That is a hypothesis to test with users and experiments, not a conclusion about competitors' private capabilities.

**How I would evaluate the next step**

The literature does not supply a single transferable leaderboard: papers vary in embodiment, task, policy, training budget, filtering fraction, and metrics. Comparing an episode-ranking score, reward-order correlation, QC AUC, and robot success percentage as though they measure the same outcome would be misleading.

I propose two separate studies:

1. **Judge the scorers.** Build a small, independently reviewed collection containing progress, mistakes, recovery, necessary waiting, repetition, ambiguous outcomes, and recording defects. Include several tasks and camera conditions. Reviewers label intervals and uncertainty without first seeing model verdicts. Split by episode and collection session, keeping neighboring frames together. Measure per-category precision/recall, false exclusion of useful segments, timing error, and inference cost. Report completion and progress separately.
2. **Judge the training intervention.** On an executable benchmark, compare all data, equal-size random selection, simple heuristics, and learned selection at matched retention. Hold policy architecture and training recipe fixed; use multiple training seeds and paired evaluation scenes. Measure robot success, task throughput, rare-case/recovery performance, and total scoring-plus-training cost. Use a separate selection-validation set and final test set. If compute efficiency is a claim, add an equal-total-compute comparison.

A random subset is essential: it distinguishes intelligent selection from benefits caused merely by changing dataset size. Report uncertainty across scenes and training seeds separately; hundreds of rollouts from one trained policy do not resolve training-seed variability.

For sequence handling, preserve original timestamps, episode boundaries, and selection masks or sample weights. Do not concatenate surviving intervals in a way that manufactures state/action transitions across removed time. This is my engineering recommendation for valid training data, independent of which scoring model wins.

**The most useful next demonstrations, in order**

1. **Task-conditioned review across multiple tasks.** Give Robometer the actual instruction, display separate progress and success tracks, and compare its judgments with reviewed evidence. The demonstration should make uncertainty and camera disagreements inspectable. This tests how far we can extend beyond one supported bottle task.
2. **Redundancy-aware selection.** Use SCIZOR as the research reference and show groups of repeated behavior, which representative segments are retained, and what happens to task/scene coverage. A visually smaller dataset is the demo outcome; improved training requires the second study above.
3. **Selection driven by a failure or target skill.** Begin with retrieval such as STRAP, then investigate DataMIL or CUPID when policy-training and evaluation artifacts are available. The user-facing question becomes “which existing demonstrations are useful for this problem?”

For an initial experiment I would prioritize **Robometer plus a carefully reviewed multi-task sample**, followed by **SCIZOR's redundancy component**. SARM2 is the strongest nearby direction when stage annotations and multi-view/state inputs are available. CUPID and DataMIL become most compelling once we can measure the effect of a recipe on a specific trained policy.

No policy training or independent paper reproduction was performed for this study. Existing application code was inspected but not modified.
