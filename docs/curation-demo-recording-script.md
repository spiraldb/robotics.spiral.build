# Curation demo recording script

Draft: September 18, 2026.

Target approximately two minutes of screen action, edited into a one-minute demo.
Trim typing, navigation, and inference waits. Show a brief Processing state before
cutting to the actual result; do not imply inference is instantaneous.

The story: inspect a demonstration, identify a questionable moment, build a
curation recipe, review the model's annotations, and exclude training chunks
without modifying the original recording.

Delivery: a natural, professional product walkthrough. Explain each action as
you take it, using clear sentences and contractions where they feel natural.
Leave space for the footage; avoid casual filler and promotional language.

## Two-minute capture plan

| Time | Screen action | Narration |
| --- | --- | --- |
| 0:00–0:15 | Start in Data. Browse two episodes and play a short segment. Add the small caption **Simulation demonstration dataset**. | “Imitation learning depends on demonstrations. But human recordings can include pauses, repeated attempts, and mistakes—even when the task ultimately succeeds. We want to be selective about which moments we use for training.” |
| 0:15–0:35 | Play one visually understandable low-progress candidate, with context before and after it. Show a second short example only if it adds something clear. | “Let's look at this interval more closely. We'd want to understand whether it's helping the task move forward. If it isn't, we should be able to leave it out of training without losing the rest of the episode.” |
| 0:35–0:45 | Switch to Curation and select the dataset. | “In Curation, we can define those choices as a workflow. I'll start with the dataset we've just been inspecting.” |
| 0:45–1:05 | Type **“Remove episodes with more than 4,000 messages”** in the natural-language field. Click the blue **Add rule** button once. Show the new editable message-count filter and its graph node, then select the node to inspect the distribution: **100 incoming, 40 excluded, 60 retained** for this filter alone. | “I can describe a rule in plain language—remove episodes with more than four thousand messages—and Spiral adds it to the workflow. The rule stays editable, and the graph shows which episodes it removes.” |
| 1:05–1:25 | Type **“Remove unproductive moments from training”**. Click **Add rule** once, inspect the new rule, and select its graph node. Let an episode open automatically. Capture Processing briefly. | “Next, I'll ask Spiral to remove unproductive moments from training. It uses a state-of-the-art model to identify these moments automatically. Selecting the step opens an episode so we can review the result.” |
| 1:25–1:45 | Show the **three synchronized camera feeds, followed by the shared timeline**. Inspect the **Progress score** track. Click an interval flagged by the automatic result to replay the same moment chosen for the opening during rehearsal. | “Spiral identifies these moments automatically. We can click a flagged interval and watch it in context across all three cameras. The workflow leaves those moments out of the training selection while preserving the original recording.” |
| 1:45–2:00 | Save the recipe. Show a genuinely completed output from this automatic recipe and its actual retained/excluded counts. Identify the evaluated episode scope; do not present a one-episode result as the whole dataset. | “After reviewing the results, I'll save the recipe. Running it applies the workflow and excludes the identified moments from the training selection. The original recordings remain intact.” |

## One-minute edited voiceover

> Imitation learning depends on demonstrations, but successful episodes can still contain pauses and repeated attempts we don't want the robot to learn.
>
> Let's review those moments in Spiral.
>
> I can describe a rule in plain language: remove episodes with more than four thousand messages. One click adds an editable filter and a graph node showing its impact.
>
> Next, I'll type “Remove unproductive moments from training” and add that rule. Spiral uses a state-of-the-art model to identify those moments automatically.
>
> The results appear below three synchronized cameras. We can replay a flagged interval in context and inspect what the automatic workflow will leave out of training.
>
> I'll save the recipe, then run it to produce the curated training selection. The original recordings stay intact.

Suggested edit: 0–12 seconds for the problem and example, 12–23 for filters,
23–34 for typing and adding the moment-removal rule, 34–50 for reviewing the
annotation, and 50–60 for saving and the resulting selection. Keep both exact
natural-language prompts and their single-click additions visible in the edit.

## Recording details

- Dataset: select `dataset_warp_sim_bottles_sample` in Studio, containing **100 simulated episodes**. Automatic moment detection in this demo is supported for this sample. Do not select `abc130k_sample` (40 episodes), which is not supported by this task-specific model. Keep the internal dataset identifier out of narration. The human-demonstration discussion motivates the problem; these videos must not be presented as real human teleoperation footage.
- Filter prompt: **“Remove episodes with more than 4,000 messages”**. Record the actual typing, the single blue **Add rule** click, and the newly added editable condition **Messages > 4000** plus graph node. The exact comma-form prompt passed interpretation-only verification as **Messages > 4000**. The single-click browser flow was verified with the equivalent wording “remove episode whose messages larger than 4000.” The resulting rule has been verified on the 100-episode sample: **40 excluded, 60 retained**. These counts apply when it is the first and only episode filter; use the live counts if the recipe changes. The earlier duration/3,000-message example does not belong in this sequence.
- Proposed transformation prompt: **“Remove unproductive moments from training”**. Click **Add rule** once. This exact wording passed interpretation-only verification. Inspect the resulting rule during rehearsal; this check did not run episode inference.
- Choose the opening example **after reviewing the automatic result**. Use an episode that reaches this transform, select a visibly flagged interval, and check the surrounding footage before recording. Describe it as **model-flagged for review** unless the images support a more specific explanation.
- The transformation uses a built-in selection setting; no threshold adjustment is part of the demo. A flagged interval and completed output under this automatic setting still need confirmation. Earlier manually tuned annotations and counts are not evidence for the current automatic behavior.
- All three cameras are shown for inspection. Only the original top-camera recording is used for model inference. The timeline is the existing shared episode timeline with an additional progress track.
- Scores for the currently displayed episode stay in viewer memory; there is no custom persistent score cache. Avoid switching episodes repeatedly during the take because each new selection can start inference.
- Record the final output counts from a completed run of the same recipe and built-in setting shown in the take. Do not reuse counts from the earlier manually tuned validation run.

## Closing language and product boundaries

The current actions are **Save** and **Run**. Save persists the recipe revision;
Run compiles and executes it to produce outputs. There is no combined
**Compile and save** action yet. If that wording is desired for the recording,
define and implement the combined behavior first rather than only changing the
voiceover or button label.

An alternative, more technical closing is:

> “I'll save the recipe. When we run it, Spiral optimizes the query, moving filters earlier where it's safe to reduce unnecessary data loading and computation.”

Safe claims include native filter pushdown, projection pruning, and moving
expensive work after independent filters where semantics permit. Do not claim
arbitrary transformation reordering, automatic reuse of repeated model calls,
or a guaranteed minimum amount of data loading. Recipe order can affect results.

The demo produces episode/chunk selections and derived camera views for
downstream use. It does not demonstrate a one-click training handoff, policy
training, or improved policy performance. Keep those separate from the curation
result shown here.
