# Curation demo recording script

Revised September 18, 2026. Target approximately two minutes of recording, edited to about 1:34. Use a natural, professional voice. Preserve real mouse motion, clicks, and character-by-character typing. Trim idle waits while showing genuine loading and inference states.

The story: define unproductive moments, show an example in a demonstration, then use curation to identify and exclude those moments from training while preserving original recordings.

## Approved narration and screen action

**0:00–0:20 — Show the example in Data.** Select episode #14, let its cameras and plots load, seek to approximately 45.5 seconds, and play through the bottle-repositioning sequence at 46–49 seconds. Pause, then click Curation. The opening uses the same episode inspected later in the transformation node.

> In this demo, we'll show how to use Spiral to remove unproductive moments—like pauses and repeated attempts—from robot demonstrations before training.

*Seek to the example and play it.*

> Take this moment, for example. Even when a demonstration succeeds, it can contain moments like these that we don't want the robot to learn.

**0:20–0:30 — Transition into an empty curation and select the dataset.**

> With Spiral's data curation workflow, we can identify and exclude those moments from training while keeping the original recordings intact.

**0:30–0:40 — Type “remove episodes shorter than 20s” and click Add.** Show the graph update from 100 to 79 episodes.

> Let's start with our dataset and add a few rules in plain English. First, remove episodes shorter than twenty seconds.

**0:40–0:52 — Type “remove episodes with fewer than 5000 messages” and click Add.** Show 79 → 17.

> Then, remove episodes with fewer than five thousand messages. The graph shows the effect of each rule.

**0:52–1:06 — Type “remove unproductive moments in each episode” and click Add.** Select the transformation and show the episode loading, followed by model processing.

> Now, remove unproductive moments in each episode. Spiral uses a state-of-the-art model to identify them automatically.

**1:06–1:10 — Show the populated progress track.**

> Here are the flagged moments.

**1:10–1:22 — Seek to the same 46–49-second interval and play it across all three cameras.**

> We can replay them across all three cameras and check the model's judgment before using the result for training.

**1:22–1:34 — Click Curated output and inspect the retained episodes and inline details.** End here without Save or Run.

> Finally, we can inspect the retained episodes and their details. The original recordings stay intact, and unproductive moments will be automatically excluded from training.

## Caption treatment

Subtitles overlay the footage directly, using short phrases with a blue highlight and subtle animation on each spoken word. No bottom banner, reserved caption strip, or added video padding. During the camera review, place captions over the empty left area so the timeline stays readable. Briefly disclose the simulated dataset and AI-generated narration in the opening.

## Recording setup and evidence

- Select `dataset_warp_sim_bottles_sample`, containing **100 simulated episodes**. This is the supported sample for automatic moment detection. Do not select `abc130k_sample` (40 episodes). Keep internal dataset identifiers out of narration, and do not present simulation footage as human teleoperation recordings.
- Start from an empty curation, not the existing prepared recipe. Add only these three rules, in the order shown. The intended filter conditions are **Duration < 20 seconds**, then **Messages < 5000**. Import metadata verifies the expected sequence **100 → 79 → 17**: the first filter excludes 21 episodes; the second excludes 62 of the remaining 79. Episodes with exactly 5,000 messages are retained. Confirm these counts in the live graph during rehearsal; previous filter examples do not apply.
- The third prompt normalizes the obvious typo in “remove unproductive memoments in each episodes” to **“remove unproductive moments in each episode”**, preserving the requested meaning.
- All three prompts and the **100 → 79 → 17** graph counts were verified in the completed native-input recording. The revised opening selects episode #14, seeks to the bottle-repositioning sequence, plays it, and visibly transitions into Curation.
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
