# Initial-frame workflow prompt experiment

Current result: the native planner bug is fixed, and the unchanged prompt
completes with frame selection before decoding. The final live measurements
are in the last section: 15.5 seconds for the main query and 101.4 seconds for
the whole assistant turn. Earlier failures below are retained as experiment
evidence.

Measured in local Studio on 2026-09-16 UTC using the same installed native driver
as the baseline in `workflow-execution-profile.md`. Both reruns started in a
new blank workflow with the exact user prompt:

> filter out episodes whose toy is initially placed below the robot arm

## Guidance and observed behavior

The system prompt now requires native frame selection before pixel decoding
for initial-frame, single-moment and sparse-sampling tasks, including pilots.
`mcap.read_video` is reserved for tasks that need the full video decoded.
The `read_topic`, `read_video` and `from_packets` discovery entries use the same
rule; the old decode-then-sample recipe was removed. Native source identities,
packet ordering, codec support and timestamp distinctions remain documented.

An initial, softer revision still chose `read_video`, with two pilots followed
by a full-cohort decode-then-filter query. After strengthening the rule and
aligning discovery text, the agent selected:

`read_topic → from_packets → frames(frame_indices([0])) → decode`

It made no `read_video` query in the second rerun. Request instrumentation
verified that the model received the full-video-only system prompt.

## Measurements

| Measurement | First revision | Full-video-only revision |
| --- | ---: | ---: |
| Assistant turn, request to idle | 148.010 s | 89.627 s |
| Submitted queries | 3 | 2 |
| Query lifecycle time, summed | 80.686 s | 3.288 s |
| First pilot | 9.291 s | 0.974 s, packet inspection |
| Second pilot | 9.059 s | — |
| Final query attempt | 62.336 s, completed | 2.314 s, failed |
| Planning calls | 3 | 7, including 5 failures |
| Total tool calls | 20 | 23 |
| Result | 32 retained episodes | No filtering result |

End-to-end timings use the saved conversation's initial user-message and idle
update timestamps. Query lifecycle times use backend `started_ms` and
`finished_ms`; successful query execution elapsed times differ by at most 1 ms.
The second run spent the remaining 86.339 seconds outside admitted query
lifecycles: model turns, discovery, planning, error recovery and UI/tool
handling. This is not a model-only timing. The temporary fetch observer's
response clones were aborted when the client finished consuming responses,
so its completion timings are not used as a reliable model/tool breakdown.

The earlier baseline decoded 105,612 frames to select 40 initial images, and
its full query measured 64–68 seconds over two EXPLAIN ANALYZE runs. The first
revision's 62-second final query is consistent with the same expensive strategy.
Its prompts and decision threshold changed, producing 32 retained episodes
versus 5 in the original saved workflow; these are not correctness-equivalent
results. The second run failed, so 89.6 seconds is time to a reported blocker,
not a speedup for completing the task. Efficient selected-frame decoding has
not yet been measured successfully on this dataset.

## Native execution blocker

The selected-frame query planned, but execution failed with:

```
scalar `spql.video.from_packets#shape_utf8_utf8` has no synchronous Vortex
expression implementation; it must be planned by a physical project
```

The agent stopped and reported the native failure instead of falling back to
full-video decoding. Before that, it made five unsuccessful planning attempts
while discovering the native list aggregate (`spql.list.collect`). The final
SQL also packs packet order as `chunk_start_offset * 1000000 + sample_ordinal`
without establishing bounds; that ordering must be validated or replaced with
a trustworthy ordinal before treating a successful run as correct. It attempted
the full cohort rather than the instructed one-episode packet-assembly pilot.
Thus the desired decoder choice and refusal to fall back worked, but the SQL
and pilot behavior are not yet fully validated.

No native engine fix or fallback was installed. The next performance comparison
requires fixing the native planner failure and validating packet ordering.

## Reproduction and validation

Both benchmark conversations and SQL were saved as separate workflows:

- First revision: `workflow-1789532884133-f5bbe83d5d1bd000`.
- Full-video-only revision: `workflow-1789533340632-bd5c3f5b6adf6000`.

Raw conversations, query snapshots, SQL, prompt versions and `measurements.json`
are in the Git-ignored `outputs/workflow-prompt-rerun-2026-09-16/` directory.
The original workflow was not overwritten. The frontend/backend `dx build`
completed, capability JSON parses, and `git diff --check` passed. Temporary
browser timing instrumentation was removed after measurement.

## Native planner follow-up

The user explicitly authorized fixing and rebuilding the sibling native engine
checkout. Generator arguments were being lowered directly to synchronous Vortex
expressions inside the generator evaluator. `video.from_packets` has an async
implementation, so a nested call failed even though the same call worked when
projected separately.

The native fix prepares computed generator arguments with the existing scalar
projection planner. Generator expansion and fused generator reduction both use
that path. Temporary argument fields are removed from the output, and generated
names avoid input/output field collisions. Studio does not rewrite the query
around this limitation.

The existing packet-video test now exercises `frames(from_packets(...), ...)`
directly. It failed with the exact Studio error before the fix and passed after
the fix, including requested display order, decoded pixels, and GOP dependency
checks. Fifteen related engine tests and two video generator catalog tests also
passed. An identical duplicate video test had to be removed because it prevented
the test crate from compiling.

The native checkout already contained unrelated uncommitted changes. This fix
adds `engine/spiral-engine/src/planner/generator.rs`, updates generator lowering
in `planner/mod.rs`, and adjusts the two video test files. The existing changes
were preserved. Raw regression logs are in
`outputs/workflow-native-fix-2026-09-16/`.

## Successful live rerun after the native fix

The first fresh assistant attempt after the engine rebuild still failed after
110.902 seconds: it searched for unqualified aggregate prefixes (`list`,
`array`, `collect`) and incorrectly concluded that packet aggregation was
unavailable. The native aggregate was available. The system prompt and
`from_packets` discovery entry now explicitly name `spql.list.collect`, give
its `STRUCT_PACK` input form, and explain qualified prefix discovery. A native
one-episode query verified `ROW_NUMBER` over `(chunk_start_offset,
sample_ordinal)` cast to `BIGINT` as a stream-wide decode ordinal.

With those docs, the unchanged user prompt completed in a new blank workflow:
`workflow-1789535142470-a75201549717000`, saved as **Filter episodes by initial
toy placement**. It used native first-frame selection, a correct stream-wide
ordinal, and zero failed tool/planning calls. It made 20 tool calls, including
two queries and two image inspections. It returned 24 retained episodes and
visually spot-checked representative retained and excluded frames; the remaining
16 episodes were model-excluded. The final displayed SQL and result were
restored to the retained set.

| Measurement | Earlier decode-all rerun | Successful native-selection rerun |
| --- | ---: | ---: |
| Assistant turn, request to idle | 148.010 s | 101.437 s |
| Main filtering query execution | 62.335 s | 15.511 s |
| All admitted query lifecycles | 80.686 s | 29.698 s |
| Time outside query lifecycles | 67.324 s | 71.739 s |
| Queries | 3 | 2 |

The main query is approximately 4× faster; total turn time is 31.5% lower.
The second successful query took 14.186 seconds to retrieve model-excluded
frames for visual review. It repeated packet processing and both score calls.
The remaining 71.739 seconds combines model requests, metadata discovery,
planning, image inspection and result handling; it is not a model-only metric.
These are observed runs, not a statistical benchmark. Scoring text and decision
rules differed between the fresh conversations, so result counts are not an
accuracy comparison.

The previously failing first-frame-only SQL also succeeds: 14.710 seconds HTTP,
11.191 seconds native execution, and 40 images. Its plan sends 40 selected
frame references to `video.decode`. MCAP processing still reads 105,612 packet
messages and reports 3,460,563,185 compressed bytes across requested chunks.
The MCAP `decoded_rows` metric here counts message deserialization, not decoded
video pixels. Supporting codec frames can still be decoded internally. The
comparable earlier unscored image query took 54.328 seconds native execution.

The loaded driver SHA-256 is
`03e6b349a48a9704ff9a9f30ebe61628dcbb9e6a016e4fa33bbb871e572866c7`.
The native release build, scoped `spiral-engine` release Clippy with warnings
as errors, Rust formatting and diff checks passed. Studio's fullstack bundle
also built successfully. The dev hot-reload bundle stalled during one rebuild;
an explicit fullstack build restored the client before the measured run.

A separate post-run EXPLAIN ANALYZE of the final SQL completed in 13.150 seconds
HTTP / 11.950 seconds native execution and returned the same 24 retained rows.
Its frame generator, pixel decoder and both score operators each process 40 rows.
This profiling query is separate from the measured assistant turn.
