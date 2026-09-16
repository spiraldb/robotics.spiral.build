# Latest saved workflow execution profile — September 16, 2026 UTC

The main bottleneck is decoding video that the query later discards. The latest
saved workflow, **Filter initial toy-below-arm episodes**, decodes 105,612 frames
from 40 episodes to select 40 initial images. Loading the saved document is fast;
query execution takes about a minute.

## Scope and method

- Workflow ID: `workflow-1789522445938-62b78a03fb3e1000`, revision 1.
- Dataset: `dataset_abc130k_sample`, `/top-camera`.
- Local Studio: port 8080, serving backend PID 89431 during measurement.
- Driver: `target/release/libadbc_driver_spiraldb.dylib`, SHA-256
  `c133a1ac5c5aa86a75b132c9aae432d9cfea6e505653e9116148971793ad5bb7`.
  The sibling Spiral checkout was at `23c8296a6`; its source was read only.
- Used existing project-scoped HTTP endpoints, native
  `EXPLAIN (ANALYZE, FORMAT JSON)`, saved conversation timestamps, and an
  eight-second macOS stack sample of the serving backend.
- Queries ran sequentially. These are individual observations on a live
  development machine, not isolated benchmark percentiles or cold-cache tests.
  No saved SQL, application code, driver, or database data was changed.

## Measurements

| Operation | HTTP wall time | Native instrumented execution |
| --- | ---: | ---: |
| List saved workflows | 1.8 ms | — |
| Read session run history (empty) | 1.3 ms | — |
| Load latest saved document and plan | 9.1 ms | — |
| Plan the latest SQL again, without executing | 2.139 s | — |
| Analyze original SQL | 68.376 s | 65.278 s |
| Analyze initial images without scoring | 57.351 s | 54.328 s |
| Analyze original SQL again | 64.258 s | 62.175 s |

Both original-query measurements produced the same 105,612 decoded frames,
3,591 time-filtered frames, 40 scored images, and five output rows. The repeat's
two scoring active-work counters were 6.813 and 6.621 seconds. The roughly
four-second variation between full runs is another reason not to infer exact
phase durations by subtracting the diagnostic query's wall time.

The unscored diagnostic preserves the original initial-frame CTE and returns
the 40 initial RGB images, ordered by episode. The original scores those images
and returns five matching rows, ordered by score margin. Different output and
materialization costs mean the difference is **not** an isolated inference
measurement. Nevertheless, most of the latency persists without inference.

The original request spent about 3.10 seconds outside the native execution
interval. This includes query setup, planning, report construction and delivery;
the existing instrumentation does not subdivide that interval. Native query
collection reported 68.356 seconds, only about 20 ms less than HTTP wall time.

## Where the work goes

The original native report gives this actual row flow:

| Stage | Input rows | Output rows |
| --- | ---: | ---: |
| Video reader | 40 episodes | **105,612 decoded frames** |
| Episode join | 105,612 frames | 105,612 frames |
| First-three-seconds upper-bound filter | 105,612 frames | 3,591 frames |
| Window and `rn = 1` filter | 3,591 frames | **40 images** |
| First image-scoring operator | 40 images | 40 scores |
| Second image-scoring operator | 40 images | 40 scores |
| Placement-score filter | 40 episodes | 5 episodes |

The three-second predicate discards **96.6%** of frames only after decoding.
Overall, the reader emits about **2,640 frames per selected initial image**.
The physical plan places the lower time bound in the join residual and the
upper time bound in a filter downstream of the reader.

This agrees with the native implementation: `log_time_comparison` in
`engine/plugins/modality/spiral-robotics/src/read_topic.rs` recognizes comparisons
with a literal time bound. The workflow uses each joined episode's
`message_start_time`, so those per-episode bounds do not become reader bounds in
this plan. `read_topic/video.rs` passes recognized bounds to the decoder.

The serving backend's stack sample showed H.264 decoding as the leading active
CPU work: `decode_residual`, `ff_h264_decode_mb_cavlc`, and `ff_h264_filter_mb`.
Its sampled physical footprint was 3.9 GB. That is process memory at one moment,
not a measurement of this query's exclusive allocation.

The two `spql.ai.score` operators recorded 6.978 and 6.834 seconds of
`facet_active_wall_work_ns`, each handling 40 images. Their physical operators
are consecutive and require `mps.compute`. The current upstream CLIP scoring
implementation caches text features but calls `get_image_features` for each
score invocation, so the two prompts repeat image encoding.

**Do not add operator active-time counters to derive a wall-time breakdown.**
They time synchronous push/poll work, not all asynchronous waits or background
decoder/inference work. In particular, the video reader's 1.100-second active
counter is not its total decoding latency. The report also lacks exact MCAP
object-transfer byte counts, so it cannot separate network waits from decoding
with precision.

RGB conversion appears before `rn = 1` in the physical plan. Because arrays can
be lazy and share backing storage, operator order alone does not prove that all
3,591 frames are eagerly converted. That needs more focused materialization
instrumentation before calling it a separate optimization.

## What happened in the original Assistant conversation

The saved turn spans **34m 20.8s**, from September 16 01:34:48.604 UTC to the
start of its final assistant message at 02:09:09.388 UTC.

| Conversation interval | Time |
| --- | ---: |
| Request to first run-inspection message | 28.1 s |
| First run-inspection message to next correction message | **31m 37.1s** |
| Corrected image-only run inspection to next message | 49.9 s |
| Final scored run inspection to next message | 62.2 s |

The first run failed with `image.to_rgb requires range metadata; supply
color_assumption for absent fields`. The Assistant then supplied
`bt709_limited`, inspected initial images, and ran the final scoring SQL.

These are **message-start intervals**, which include model generation and tool
work, not exact tool durations. The run history had been cleared by a server
restart, and the persisted conversation lacks tool completion timestamps.
Consequently the 31-minute interval is real but cannot be attributed entirely
to execution, downloads, model latency, or browser suspension from this data.
It should be investigated separately from the reproducible minute-long query.

## Follow-up priorities

1. Evaluate the existing native frame-selection path before proposing a new
   engine capability. A live catalog check confirmed public
   `spql.video.frame_at` overloads for media, streams, and packet sources, plus
   `spql.video.from_packets` and `spql.video.decode`. Studio documents
   `spql.video.frames` as well. These select frame references before pixel
   decoding; MCAP packet assembly must preserve original ordering, timestamps,
   and codec support. This alternative has not yet been benchmarked against this
   dataset. If retaining `mcap.read_video`, investigate native per-episode
   time-bound propagation to address the measured 105,612-to-40 mismatch.
2. Reuse image features across the two CLIP prompts in the native inference
   implementation. Measure the improvement after fixing excessive decoding.
3. Record model-request, tool-start/tool-finish, and query lifecycle timestamps
   so an anomalous 31-minute failed attempt is attributable next time.

These are proposed native fixes and instrumentation work. No SQL workaround,
new cache, or concurrency change was installed.

## Agent capability discovery follow-up

The latest saved agent trace queried documentation only for
`spql.mcap.read_video` and `spql.image.to_rgb`. It never looked up or used
`spql.video.frame_at`, `spql.video.frames`, or `spql.video.decode`.
Those capabilities are discoverable through `inspect_functions`; the live
scalar catalog includes `frame_at`. Only AI functions are automatically supplied
with each turn. Studio's camera instructions explicitly prefer
`mcap.read_video`, and the packet-source documentation repeats that preference.
Thus the guidance steers toward the path that was slow here even though native
selection-before-decoding capabilities exist. Runtime registration is verified;
successful, efficient application of the packet path to this dataset is not.

## Evidence

Local raw reports, diagnostic SQL requests, the serving process's stack sample,
and a compact saved-conversation timeline are in
`outputs/workflow-profile-2026-09-16/` (Git-ignored). The analyzed SQL is preserved
there as `workflow.sql`. The native full report includes schemas, physical plans,
operator counts, timings, and the actual five-row output count.
