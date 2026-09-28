# Demo guide

Every demo has one purpose: show that Spiral Robotics solves a real problem easily. Every sentence and every moment serves that purpose. Stay focused; never wander into side features, background, or anything else.

Human attention is extremely scarce, so a demo never wastes a moment of it.

## Time

- **Earn every second.** Cut what doesn't move the story forward. Pauses and animations are not waste: they let the eye land and make the video feel natural. A dead wait is waste.
- **Pace by importance.** What matters plays at normal speed, with room to read it. Secondary steps (choosing a dataset, renaming, opening menus, basic filters) move quickly. Model waits and loading become short, eased time-lapses.
- **No jump cuts, no transitions.** Compress time with eased speed ramps. Never let the cursor teleport, and never use dissolves or fades to hide a join.

## Words

- **Every sentence must be useful.** If removing it loses nothing, remove it.
- **Concrete over abstract.** Name what's on screen: "reach, grab, carry, drop", not "a complete task". Give the viewer an image, not a claim.
- **Hook first.** In the first seconds, show the problem so viewers want to stay: a concrete failure, and why it matters.
- **Sound like a person, never like an LLM.** Talk the way someone demoing to a colleague talks: plain sentences, contractions, ordinary words. No dramatic colons, rule-of-three lists, "not just X, but Y", "ready to…, ready for…" cadence, or buzzwords like "seamlessly", "powerful", "unlock". Read every line aloud; if nobody would say it, rewrite it.
- **True to the screen.** Every number, label or claim in the narration must be visible in that take. Model output varies between runs, so check each take.

## Attention

Studio's UI is dense, and a viewer who doesn't know it gets lost. Always show them where to look, in the spirit of Screen Studio.

- **Zoom to the action.** Smooth, spring-eased zooms and pans follow the cursor into the part of the screen that matters (a node, a slider, a result, a number). Then return to the full frame so the viewer keeps their bearings. Fast moves get a touch of motion blur.
- **Move the camera as little as possible.** Every camera move costs the viewer their bearings. If a spotlight alone shows the subject, don't zoom. Never chain zooms back to back, and end every video on the full app so the viewer sees where they landed.
- **Spotlight the subject.** When one element is the point (the flagged stretch, "Found in 11 of 11 episodes", the Published flag), dim the rest of the frame slightly or outline the element. At most one spotlight at a time.
- **Make the cursor easy to follow.** Smoothed paths, a slightly larger cursor, and a soft ripple on each click that matters.
- **Short labels when needed.** A small callout can name what the viewer is looking at when the narration alone isn't enough. Keep it to a few words, then remove it.
- **Frame the app.** Show the app on a quiet backdrop with padding, rounded corners and a soft shadow, and keep its colors exactly as they appear in the browser.
- **Subtitles.** Overlay short phrase groups on the footage, with an animated lime highlight on each word as it's spoken. No bottom banner. Place them where they don't cover the action.

Every effect has to earn its place: it guides the eye to what the narration is talking about, or it goes.

## Story

Every demo follows the same arc: a concrete problem → curate it in Spiral → publish → open the result in Data. The last shot proves the result.

## Workflow

Record with the cut in mind, and never redo what didn't change.

- **Shots, not takes.** Each video is a sequence of shots, split at natural rest points (e.g. the intro playback, building the graph, the model step, inspecting results, publishing, checking Data). Each shot is recorded, logged and chosen on its own, and a change re-records only the shots it touches.
- **Seamless joins, no transitions.** The video must feel like one continuous recording: no dissolves, fades or wipes between shots. Each shot starts exactly where the previous one ended, on the same screen with the cursor at the same pixel. If a shot ends with the cursor on the Curation button, the next begins with it right there.
- **Errands happen off camera.** Steps the viewer doesn't need to see (naming the curation, closing a leftover tab) go into the next shot's setup, before recording starts, not into the video.
- **Coverage for model steps.** Model output varies. Record a few variants of those shots once, and pick the one that fits the narration, instead of re-taking the whole video.
- **Narration is independent of picture.** Voice lines are separate clips pinned to logged moments. Changing words re-renders those lines and re-times the edit; it never needs a new recording.
- **Fast previews.** Review from a quick render; do the full-quality render and checks only for the final.
- **One owner per change,** from shot to final render.

## Voice and polish

- Voice: Gemini 3.8 Flash TTS, voice Umbriel, "pro-grounded" direction (a product lead demoing to a customer: calm, clear, never salesy). See `exports/demo-series-2026-09-26/voice-tests/README.md`.
- The product on screen must look finished. Fix rough UI in the app itself; don't hide it in the edit.
