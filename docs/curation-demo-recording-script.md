# Curation demo recording script

Revised September 21, 2026. **Target: approximately 64 seconds, guided version only.** Use a warm, professional male voice, like someone explaining the product while using it. Keep delivery natural and remove loading waits and excess audio silence before accelerating speech.

Keep native mouse movements, clicks, and character-by-character typing. Let actions happen under the narration. Move briskly through the two basic filters, then give the transformation and half-speed replay more attention. The timings below are rounded from the final edit; cuts follow the spoken delivery without interrupting sentences.

## Narration and screen action

**0:00–0:10 — Open the episode in Data.** Select episode #14 and let its cameras and plots load during the opening. Cut two seconds from this initial loading wait while preserving the selection and loaded result. Move toward the bottle sequence.

> Adding more episodes to your training data can actually make your model perform worse. This is because even successful episodes can contain human mistakes, hesitations, and repeated attempts.

**0:10–0:21 — Show the example.** Seek to about 45.5 seconds and play the bottle-repositioning sequence. Guide attention to the gripper and bottle. Bring in the bottom-right paper citation on “Adding these moments to…” and keep it visible through the end of that sentence.

> Take this moment, for example. The gripper keeps adjusting around the bottle instead of moving it into the bin. Adding these moments to the training set wastes training resources and can degrade model performance.

**0:21–0:25 — Start an empty curation.** Open Curation and select the intended dataset. Keep the native selection action visible.

> Let's remove those moments in Spiral.

**0:25–0:34 — Add both basic filters, then inspect the graph.** Type “remove episodes shorter than 20s” and click Add, then immediately type “remove episodes with fewer than 5000 messages” and click Add. Accelerate the recorded typing and pointer movement to about 1.6× for these two inputs. Complete both Add clicks, then smoothly zoom out to the full graph and show 100 → 79 → 17. Allocate exactly nine seconds: 3.2 seconds for the duration rule, 4.2 for the message-count rule, and 1.6 for the graph result.

> We'll start with simple measurements—episode duration and message count. Spiral automatically removes episodes that don't qualify.

**0:34–0:44 — Add the main transformation.** Type “remove unproductive moments in each episode” and click Add. Show a brief genuine processing state, then the populated progress track. Guide attention to the flagged region. Give “within” a clear, natural vocal emphasis, with a tiny pause before it, to mark the move from filtering whole episodes to filtering sections inside them. Keep the captions in normal sentence case.

> Now let's filter out the hesitations and repeated attempts **within** each episode. Spiral uses a state-of-the-art model to identify those sections and exclude them from training.

**0:44–0:55 — Inspect the flagged interval.** Seek to episode time 46 seconds and replay 46–49 seconds at 0.5×, without a speed-label overlay. Keep all three cameras and the timeline visible. After the six-second replay, keep the flagged interval in view while explaining that only those sections are excluded.

> Here's the section it flagged. Spiral automatically identifies these moments and excludes only those sections from training, so the rest of the episode is still usable.

**0:55–1:04 — Inspect the output and conclude.** Click Curated output and briefly show the retained episodes and inline details. Begin the optimization sentence immediately, zoom back out by 0:58.5, then click Save and show the actual Saved state. Do not narrate the Save action or add a separate end-screen hold.

> Spiral automatically optimizes I/O, network transfers, and measurement costs—so you can focus on the data your robot learns from.

## Paper citation overlay

Show this citation in the bottom-right corner exactly as the narrator begins “Adding these moments to the training set wastes training resources and can degrade model performance.” Keep it visible for the full sentence, then fade it out when entering Curation. Synchronize its entrance to those spoken words, not to the start of the video:

> Yu et al., WARP-RM (2026)
> https://arxiv.org/abs/2606.28320

Use only the paper citation: no spoken research result, percentage callout, or separate research shot. Overlay it directly on the footage, with safe margins and sufficient contrast, separate from the animated subtitles. Keep the model acronym out of spoken narration.

## Guided edit and captions

Use short, restrained zooms and blue highlights for rule entry, graph counts, and the flagged interval. Return to the full three-camera view for the half-speed replay. Cut two seconds from the initial Data-view load as well as shortening the curation processing wait. Retain a brief genuine loading state and the populated result. These are editorial time cuts, not a measured loading-speed improvement. Shorten transitions and pauses around the first two filters, retaining their typing, Add clicks, and resulting graph updates.

Subtitles overlay the footage in short phrase groups with an animated blue highlight on each spoken word. No bottom banner or video padding. Place replay captions over unused sidebar space so the timeline remains readable. Do not add the “Simulation dataset · AI-generated voice” corner overlay. Keep the requested paper citation timed to “Adding these moments to…”.

## Research basis for the opening

Sources checked September 21, 2026: [paper v4, Sections 4.2–4.3 and 4.7, Tables 1 and 5](https://arxiv.org/html/2606.28320v4), and the [project page](https://uynitsuj.github.io/warp-rm/).

- The opening is conditional: admitting longer, less efficient successful recordings degraded ordinary behavior cloning in the paper's folding experiment. This does not mean adding any data necessarily hurts.
- The citation supports the data-quality motivation. This edit makes no numerical performance claim and does not imply that this Studio walkthrough measured a training improvement.
- The example narration describes visible adjustments. Do not replace it with a specific failed-grasp or operator-error claim unless the chosen clip clearly establishes that event; a negative score alone does not.

## Recording setup and evidence

- Select `dataset_warp_sim_bottles_sample`, containing **100 simulated episodes**. This is the supported sample for automatic moment detection. Do not select `abc130k_sample` (40 episodes). Keep internal dataset identifiers out of narration. The project identifies the source as human-teleoperated simulation data; do not present it as physical-robot footage. No dataset or voice disclosure overlay is included in this edit.
- Start from an empty curation, not the existing prepared recipe. Add only these three rules, in the order shown. The intended filter conditions are **Duration < 20 seconds**, then **Messages < 5000**. Import metadata verifies the expected sequence **100 → 79 → 17**: the first filter excludes 21 episodes; the second excludes 62 of the remaining 79. Episodes with exactly 5,000 messages are retained. Confirm these counts in the live graph during rehearsal; previous filter examples do not apply.
- The third prompt normalizes the obvious typo in “remove unproductive memoments in each episodes” to **“remove unproductive moments in each episode”**, preserving the requested meaning.
- All three prompts and the **100 → 79 → 17** graph counts were re-verified in the September 20 native-input recording. The revised opening selects episode #14, seeks to the bottle-repositioning sequence, plays it, and visibly transitions into Curation.
- Use **episode #14** (`01a0b638-a04a-710f-84b8-16a3a84c0a7a`; source episode 46), **60.53 seconds**, **7,268 messages**. It becomes the first eligible episode under these filters, so a new curation selects it automatically. An existing manual selection can persist if it is still eligible; start fresh as scripted.
- The current Studio model returned all **1,817 frames** for this episode. Its red chunks are **29–30 seconds** and **46–49 seconds**. Use the longer second region for the demo: its three one-second chunks have mean progress rates of approximately **−0.041, −0.274, and −1.607**. The footage shows bottle repositioning/regrasping; the model flags it as low progress, but this is not independently labeled proof of an error.
- Review the [three-camera excerpt, 44–53 seconds](../exports/demo-candidates-2026-09-18/episode-14-review-44-53s.mp4) before recording. Seek on the timeline to 45.5 seconds and press Play to cover all three adjacent red chunks; clicking a single chunk may play only that chunk and its short context.
- A recording rehearsal exposed a 1,000-row SQL preview cap in episode score collection. The scoring path now collects its bounded complete result. Regression tests and the backend check passed, and the previously failing 1,019-frame episode loaded its full score track in the live UI.
- All three cameras are for inspection; the original top-camera recording supplies the model input. Scores for the current episode remain in viewer memory. Switching episodes can start new inference, so establish the example before the take.

## What the final shot demonstrates

Curated output shows the retained-episode list and inline details below the graph. Selecting it does not itself save, execute, or export the recipe. The final pickup shot clicks Save and verifies Saved. Save persists the recipe; execution prepares its outputs. The closing describes reuse to prepare training data, not policy training or a measured improvement in policy performance.

The engine implements filter pushdown, projection pruning, costly-scalar placement, and local scalar-expression reuse. The closing names I/O, network transfers, and measurement costs; it does not promise globally minimal cost. Optimization happens at execution planning, not merely on saving. This recording does not benchmark those savings or demonstrate scoring every retained episode.
