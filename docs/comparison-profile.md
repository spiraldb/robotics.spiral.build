# Comparison latency profile — 2026-09-15

## Latest result: both UUID fixes pass; complete dashboards in 39–46 seconds

Rebuilt the user's updated local patch on `23c8296a6` in 1m 31s, restarted Studio, and verified the server loaded the new library. SHA-256: `c133a1ac5c5aa86a75b132c9aae432d9cfea6e505653e9116148971793ad5bb7`. No upstream source was edited. The isolated captured-input JOIN matched 52 episodes in 1.096 seconds, and UNION ALL returned all 104 rows in 1.091 seconds. **The previously reported UUID blockers are resolved in this local build.**

Two full browser runs compared the same saved toy-side workflow with All episodes. Both completed with **49 selected versus 52 baseline episodes**, five populated cards, full measurement coverage, and no failed cards. No competing benchmark queries or source edits ran during either measurement.

| Measurement | Run 1 | Run 2 |
| --- | ---: | ---: |
| Documents and catalog | 0.460 s | 0.218 s |
| Execute workflow once | 14.973 s | 13.769 s |
| Dataset inventory, overlapping execution | 0.413 s | 0.388 s |
| First model call | 4.875 s | 3.812 s |
| Parallel discovery batch, slowest request | 1.512 s | 1.367 s |
| Final model plan | 15.012 s | 14.062 s |
| Population query and version checks | 1.909 s | 1.689 s |
| **First card ready, from Compare** | **38.8 s** | **34.9 s** |
| First physical-signal card ready | 41.5 s | 37.3 s |
| Four physical cards, overlapping execution | 6.951 s total | 4.197 s total |
| **All cards ready, from Compare** | **45.708 s** | **39.121 s** |

First-card times use the population completion timestamp, immediately before publishing the initial duration card. They measure result readiness, not a separately instrumented browser paint. Console delivery timestamps differ slightly from the in-app elapsed counters; milestone times are rounded accordingly. Runs began at 2026-09-15 23:58:44 UTC and 23:59:42 UTC. These are two local observations, not percentile or cold-cache benchmarks.

### Card work

Run 1 measured duration, maximum follow error, follower elbow peak temperature, gripper travel and elbow travel. Its four physical cards took 2.706, 4.313, 4.390 and 6.951 seconds, respectively. Run 2 measured duration, follower peak current, peak temperature, minimum voltage and gripper excursion; physical cards took 2.411, 2.543, 2.685 and 4.197 seconds. All four began together in each run, respecting the existing concurrency limit.

The agent chose different metrics in the two runs, so their total times are not a controlled same-query benchmark. Both showed the same duration means (7.227 s versus 7.113 s) and population counts. Run 2's recorded temperature means were 87.4 °C versus 89.2 °C, with suspicious 150 °C readings disclosed in the UI.

Backend logs separate connection/input binding from query execution and draining. Binding took **95–99 ms per dashboard query**. Physical-card query execution/draining took **1.592–5.027 seconds**; frontend card timings additionally include version checks and, for named joints, metadata validation. The input-binding step is a small part of the current wait.

### Current diagnosis

Workflow execution plus model calls account for **76–81% of total completion time**. The dashboard waits 35–39 seconds before its first card, then finishes the physical cards in another 4–7 seconds. Raising card concurrency would not address that initial delay.

The next useful performance work is to overlap independent dataset/sensor discovery with workflow execution and reduce the final planning response latency. Full planning still needs actual output schemas and previews. The existing video workflow's cost remains relevant; see the native profile below. No cache, model setting, workflow SQL, database or concurrency change was made for these successful measurements.

Evidence: `/tmp/comparison-profile-union-fixed-run1.json`, `/tmp/comparison-profile-union-fixed-run2.json`, matching `-ui.txt` files, `/tmp/comparison-profile-uuid-union-fixed.json`, `/tmp/comparison-union-fixed-serve.log`, and `/tmp/comparison-profile-union-driver-patch.diff`. Earlier blocked attempts below are retained as historical context and are superseded by this section.

## Earlier local driver: join fixed, UNION still blocked

Rebuilt the release ADBC driver from the user's current local patch on top of `23c8296a6`, restarted Studio, and verified the server mapped the rebuilt library. Build succeeded in 1m 34s. Library SHA-256: `e0479e57d77800e806a4410e2070a99e0b5de8eae5f8e085f545fd9228193e19`. No upstream source was edited.

The UUID join now works: an isolated captured-input join matched all **52 episodes**, taking 1.192 seconds initially and 0.890 seconds in the final minimal reproduction. However, `UNION ALL` still rejects generic UUID versus UUIDv7. The dashboard combines both populations with UNION, so the full card path remains blocked.

### Clean browser run, 23:47:27 UTC

| Phase | Wall time |
| --- | ---: |
| Documents and catalog | 0.410 s |
| Execute workflow once | 15.628 s |
| Dataset inventories, overlapping execution | 0.449 s |
| First model call | 3.983 s |
| Four parallel discovery queries, slowest | 1.191 s |
| Final model plan | 29.499 s |
| Population query, failed at UNION | 1.239 s |
| **Total** | **51.964 s** |

No card queries started, so first-card and final-card latency are still unmeasured. This is a failure duration, not a successful dashboard benchmark. Approximately 49.1 seconds were spent in serial source execution and model calls alone.

Two earlier patched-driver runs stopped after 44.371 and 57.461 seconds because the agent declared a duration metric but omitted `duration_seconds` from its normalization projections. The second also copied the source inference SQL into its normalizer. The comparison tool schema and instructions now explicitly require every declared metric column and prohibit redefining the captured workflow CTEs. The clean run respected that contract and exposed the remaining UNION incompatibility. This clarification does not enforce structural SQL validation; no automatic native-error retry was restored.

One attempt was excluded after a browser WASM crash: backend capture finished in 15.767 seconds, but the page did not process its response. The clean run above followed a full development-server restart. Temporary failed-SQL console logging was removed after capturing the reproduction.

### Minimal remaining failure

Capture `SELECT id AS episode_id FROM dataset_default` through the comparison input API, then execute on the connection that binds that captured result:

```sql
-- Passes: n = 52
SELECT COUNT(*) AS n
FROM comparison_input_left c
JOIN spql.mcap.episodes('dataset_default') e ON c.episode_id = e.id;

-- Fails: generic UUID versus UUIDv7 in set-operation column 1
SELECT episode_id FROM comparison_input_left
UNION ALL
SELECT id AS episode_id FROM spql.mcap.episodes('dataset_default');
```

The reproduction releases its temporary input afterward. Raw local evidence: `/tmp/comparison-profile-uuid-minimal.json`, `/tmp/comparison-profile-patched-clean.json`, `/tmp/comparison-profile-patched-population.sql`, and `/tmp/comparison-profile-driver-patch.diff`. Earlier patched runs are in `/tmp/comparison-profile-patched-run1.json` and `/tmp/comparison-profile-patched-run2.json`. No casts, SQL workaround, database changes or concurrency changes were added.

The next upstream requirement is compatible UUID types in set operations. The performance priorities remain overlapping independent discovery with workflow execution and reducing the final model-plan latency; the measurements below from the original driver remain historical context.

## Finding

The current demo waits for an expensive workflow, then several model turns, before it can render any cards. A native UUID join failure additionally triggered repeated model attempts, hiding the failure for another 74 seconds. The four-card concurrency limit was not the dominant bottleneck in the measured run.

The retry behavior is fixed: an accepted plan's execution error now stops analysis and displays the actual error. At the original measurement, full episode comparisons required the upstream generic-UUID/UUIDv7 join fix. The updated-driver results above supersede that blocker; this change adds no SQL workaround.

## Method

- Local full-stack development server, browser manually reloaded after rebuilding.
- Workflow A: **Filter episodes — toy initial table side**, revision 10, saved ID `demo-top-camera-comparison-20260915`. Workflow B: **All episodes**.
- Live dataset: 52 episodes. The workflow classifies the earliest top-camera frame using two image-scoring calls and returns 52 decision rows.
- Browser `comparison_profile` timings measure awaited wall time for source capture, model calls, discovery, population queries and card execution. Parallel intervals overlap and must not be added.
- Native query benchmarks ran separately from the browser comparison. These are individual local observations, not cold-start measurements or percentile estimates.
- Driver: local build from Spiral `23c8296a6`, including UUID scalar-comparison and UNNEST fixes, but still rejecting the Arrow-reloaded UUID join.

## Browser critical path

Baseline started at 23:11:33 UTC; verification started at 23:16:01 UTC.

| Phase | Baseline | After terminal execution errors + reader template |
| --- | ---: | ---: |
| Load saved documents and catalog | 0.478 s | 0.686 s |
| Execute workflows once | 22.627 s | 17.181 s |
| Dataset inventory, overlapping source execution | 0.449 s | 0.438 s |
| Model time before first plan | 28.636 s / 3 turns | 27.245 s / 2 turns |
| First population query | 1.552 s, failed | 1.390 s, failed |
| Total time until analysis stopped | **129.008 s** | **48.027 s** |

The baseline first failed at approximately 54.6 seconds, then spent another 74.4 seconds attempting recovery. It made 12 model calls in total (93.597 seconds of model time) and 31 discovery calls. The final attempt failed a UUID set-operation check instead of fixing the incompatible join. No card queries began.

After the change, five sample queries ran concurrently between the two model turns (slowest 1.513 seconds). The agent submitted its plan once; the population query failed once; the exact UUID error appeared immediately. No extra model turn followed execution failure.

**48 seconds is time to a clear failure, not time to a working dashboard.** The two runs also have different workflow and model timings, so the entire 81-second difference cannot be attributed to the code change. The baseline's 74-second recovery tail directly demonstrates the avoidable delay.

## Representative native queries

| Query | Full request wall time | Returned rows |
| --- | ---: | ---: |
| Per-episode maximum recorded temperature | 1.345 s | 52 |
| Per-episode shoulder-pan motion range | 1.969 s | 52 |
| Complete temperature histogram comparison | 3.501 s | 9 |
| Four simultaneous histogram comparisons | **3.325 s total** | 9 each |

The concurrent requests individually took 3.096–3.320 seconds. The histogram fixture compares 15 long recordings with 52 baseline episodes, preserving native UUIDs throughout. These measurements isolate valid sensor/aggregate queries; they do not exercise the failing Arrow join, per-card version checks, browser rendering or the complete UI card path.

Native `EXPLAIN (ANALYZE, FORMAT JSON)` provided a second view:

- Original toy-side workflow: 14.548-second request, 12.180-second instrumented execution, 52 output rows.
- It reads **10,990 video frames**, sorts/windows them, keeps the first frame per episode, then runs two `spql.ai.score` operators on 52 images each.
- A diagnostic query that keeps the same earliest-frame selection and computes mean luminance, without image scoring, still took **13.739 seconds**. This indicates substantial video/frame-processing cost; it does not isolate a precise inference duration because the queries and runs differ.
- Histogram: 4.172-second analyzed request, **0.222-second** instrumented execution, 9 output rows. The remaining time includes connection, binding/planning, initialization outside the measured execution and report construction. It is not valid to label all of that time SQL execution or rendering.
- Operator `active_wall_work_ns` counters are not a complete wall-time breakdown, particularly for asynchronous work. They must not be summed or used to claim that video reading took only the reader's active counter duration.

## Changes made

1. Added phase timing logs in the browser, including model rounds and individual cards. They log phase names and errors, not prompts or result payloads.
2. Separated plan decoding/validation from execution. Malformed plans can still be corrected; once an accepted plan executes, a native failure ends the attempt instead of entering the model repair loop. Individual card failures retain the existing independent failure behavior.
3. Supplied the native topic-reader sample-query template in initial context. The verified run skipped the earlier function-contract discovery turn.

Workflow outputs still execute once per comparison and remain in memory. No database changes, persistent cache, model changes or increased concurrency were introduced.

## Next performance work, in priority order

1. **Resolve the upstream UUID join blocker.** Then repeat the full UI measurement through the first card and final card. The current run cannot establish end-to-end card latency.
2. **Overlap independent discovery with workflow execution.** Currently no model work starts until source capture finishes. Dataset/schema discovery can start while the 17–23-second workflow runs. Final interpretation still needs actual output schemas and previews; do not assume arbitrary workflows have an episode-filter contract.
3. **Reduce planner round trips and output size.** Provide verified topic schemas/representative values directly where native inspection exposes them. The final plan response alone took 20.151 seconds in the baseline's first attempt and 23.286 seconds in verification. Measure time to first response and tool-argument generation separately before changing model configuration.
4. **Profile query setup after the join is fixed.** The representative histogram's request time greatly exceeds its instrumented execution time. Measure connection/binding/initialization independently before adding caches or changing query structure.
5. **Inspect the workflow's video access pattern with upstream.** It currently processes 10,990 frames to select 52. Any proposed native first-frame access change must preserve earliest-frame semantics; no workaround or saved-workflow rewrite was applied here.

## Local evidence

Temporary raw captures on the profiling machine:

- `/tmp/comparison-profile-baseline.json`
- `/tmp/comparison-profile-after.json`
- `/tmp/comparison-profile-native.json`
- `/tmp/comparison-profile-analyze-source-result.json`
- `/tmp/comparison-source-explain.json`
- `/tmp/comparison-profile-analyze-card-result.json`
- `/tmp/comparison-profile-initial-frames-result.json`

These contain local measurements and are not checked into the repository. Browser verification confirmed the terminal error and absence of further model retries. Frontend comparison tests cover the existing dashboard result behavior; compilation and lint checks accompany the timing/error-handling changes.
