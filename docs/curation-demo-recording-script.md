# Curation demo recording script

Draft: September 18, 2026.

Target approximately two minutes of screen action, edited into a one-minute demo.
Capture real loading states; allow the take to run longer if loading or inference
needs more time. Trim waits in the edit without implying an instantaneous result.
Use a natural, professional tone and leave space for the footage.

The story: inspect a demonstration, build a new curation from three plain-language
rules, review an automatically identified interval, and finish by inspecting the
retained episodes. Original recordings remain intact.

## Two-minute capture plan

| Time | Screen action | Narration |
| --- | --- | --- |
| 0:00–0:20 | Start recording in **Data**. Click **one episode** and wait for its data, camera feeds, and plots to load. Press **Play**, let it run for about **one second**, then pause. Keep a small **Simulation demonstration dataset** caption visible. | “I'll open a demonstration, wait for its cameras and plots, and play a moment. Even successful demonstrations can contain pauses and repeated attempts we may not want the robot to learn.” |
| 0:20–0:30 | Click **Curation** directly after that brief playback. Create a **new, empty curation** and select the supported 100-episode bottles dataset. Show that no rules are present. | “I'll start with an empty curation and select the dataset.” |
| 0:30–0:48 | Type **“remove episodes shorter than 20s”**. Click the blue **Add rule** button once. Show the new editable duration filter and graph node. Pause briefly as the graph updates; capture its transition if present. | “First, I'll ask it to remove episodes shorter than twenty seconds. One click adds the rule, and the graph shows its effect.” |
| 0:48–1:05 | Type **“remove episodes with fewer than 5000 messages”**. Click **Add rule** once. Show the second filter and pause on the updated graph and its actual counts. The expected graph transition is **79 → 17**: this rule excludes 62 of the remaining episodes. | “Then I'll remove episodes with fewer than five thousand messages. The second rule joins the same workflow.” |
| 1:05–1:20 | Type **“remove unproductive moments in each episode”**. Click **Add rule** once, then select the new **Remove unproductive moments** node. | “Next, I'll ask Spiral to remove unproductive moments in each episode.” |
| 1:20–1:38 | Confirm that **episode #14** is selected automatically as the first eligible episode. Show it loading first, with its three synchronized camera feeds. Separately show the progress score loading, then the actual populated timeline. Do not skip straight from rule creation to finished annotations. | “Spiral uses a state-of-the-art model to identify these moments automatically. Once the episode and its scores are ready, we can inspect the result.” |
| 1:38–1:52 | In episode #14, seek to approximately **45.5 seconds**, just before the **red interval at 46–49 seconds**, and play through it to about 50 seconds. Show the bottle repositioning across all three cameras, with the timeline visible. These are model-flagged chunks selected for exclusion; do not present the model judgment as a confirmed demonstration error. | “Here, the model has flagged a few seconds. We can replay the bottle repositioning from all three cameras before using the result for training.” |
| 1:52–2:00 | Click **Curated output**. Show the episode list on the left; select an episode to display its details inline on the right, with the graph still above. End here. | “Finally, Curated output lets us inspect the retained episodes and their details. The original recordings stay intact.” |

The desired recording beat after each addition is an animated graph update followed
by a short pause. The current implementation updates nodes immediately and disables
node animation. An insertion animation remains an implementation requirement if
it is essential to this shot; until then, capture the actual update and pause.

## One-minute edited voiceover

> I'll open one demonstration, wait for the cameras and plots, and play a moment. Even successful demonstrations can contain pauses and repeated attempts we may not want the robot to learn.
>
> Now, in Curation, I'll start empty and select our dataset.
>
> First, I'll type “remove episodes shorter than 20s” and add the rule. Then I'll remove episodes with fewer than five thousand messages. Each addition updates the workflow.
>
> Next: “remove unproductive moments in each episode.” Spiral uses a state-of-the-art model to identify them automatically.
>
> We'll let the episode load, then its progress scores. Here are the flagged moments. We can replay the bottle repositioning across all three cameras and inspect the result.
>
> Finally, I'll open Curated output: retained episodes on the left, selected episode details on the right. The original recordings stay intact.

Suggested edit: 0–10 seconds for opening one episode in Data, loading, and the
one-second playback; 10–16 for switching to an empty curation; 16–31 for typing
and adding both filters; 31–39 for the third rule; 39–52 for the separate loading
states and interval playback; and 52–60 for Curated output. Keep all three typed
prompts and their single **Add rule** clicks visible.

## Recording setup and evidence

- Select `dataset_warp_sim_bottles_sample`, containing **100 simulated episodes**. This is the supported sample for automatic moment detection. Do not select `abc130k_sample` (40 episodes). Keep internal dataset identifiers out of narration, and do not present simulation footage as human teleoperation recordings.
- Start from an empty curation, not the existing prepared recipe. Add only these three rules, in the order shown. The intended filter conditions are **Duration < 20 seconds**, then **Messages < 5000**. Import metadata verifies the expected sequence **100 → 79 → 17**: the first filter excludes 21 episodes; the second excludes 62 of the remaining 79. Episodes with exactly 5,000 messages are retained. Confirm these counts in the live graph during rehearsal; previous filter examples do not apply.
- The third prompt normalizes the obvious typo in “remove unproductive memoments in each episodes” to **“remove unproductive moments in each episode”**, preserving the requested meaning.
- The duration and transformation prompts were verified in the earlier live recording. The revised **“fewer than 5000 messages”** wording and the new graph counts still need a live rehearsal. The counts above were checked against the imported metadata; they are not a claim that this revised recipe has already been recorded.
- Use **episode #14** (`01a0b638-a04a-710f-84b8-16a3a84c0a7a`; source episode 46), **60.53 seconds**, **7,268 messages**. It becomes the first eligible episode under these filters, so a new curation selects it automatically. An existing manual selection can persist if it is still eligible; start fresh as scripted.
- The current Studio model returned all **1,817 frames** for this episode. Its red chunks are **29–30 seconds** and **46–49 seconds**. Use the longer second region for the demo: its three one-second chunks have mean progress rates of approximately **−0.041, −0.274, and −1.607**. The footage shows bottle repositioning/regrasping; the model flags it as low progress, but this is not independently labeled proof of an error.
- Review the [three-camera excerpt, 44–53 seconds](../exports/demo-candidates-2026-09-18/episode-14-review-44-53s.mp4) before recording. Seek on the timeline to 45.5 seconds and press Play to cover all three adjacent red chunks; clicking a single chunk may play only that chunk and its short context.
- A recording rehearsal exposed a 1,000-row SQL preview cap in episode score collection. The scoring path now collects its bounded complete result. Regression tests and the backend check passed, and the previously failing 1,019-frame episode loaded its full score track in the live UI.
- All three cameras are for inspection; the original top-camera recording supplies the model input. Scores for the current episode remain in viewer memory. Switching episodes can start new inference, so establish the example before the take.

## What the final shot demonstrates

**Curated output** already supports the retained-episode list and inline details
below the graph. Clicking it inspects the current recipe's episode selection; it
does not itself execute the recipe or create a persisted training export. Reviewing
one episode's progress scores does not prove every retained episode was scored.

The requested recording ends at this inspection view. Do not append Save or Run
to the walkthrough. If execution is shown in a separate demo, distinguish those
actions: **Save** persists the recipe, and **Run** executes it to produce outputs.
This walkthrough does not demonstrate policy training or improved policy performance.
