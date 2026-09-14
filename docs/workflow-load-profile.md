# Workflow load profile — September 13, 2026

The measured delay is primarily native database work. Studio renders the graph
within tens of milliseconds after the plan response arrives. The Studio fixes
remove avoidable work, but do not make the native requests fast.

## Measurement

Profiled `/workflow` on the local release server at `127.0.0.1:8080`, using the
most recently saved workflow, **Gripper-state events with camera evidence —
screwdriver bin** (39 displayed stages). Storage uses the existing hosted Spiral
project and the locally built ADBC driver.

A temporary script in the generated HTML used Resource Timing for requests and
a MutationObserver for the first visible `.workflow-plan-operator`. The endpoint
was recorded after two animation frames, as an approximation of first graph
paint. This is a navigation-to-visible-graph measurement, not LCP or INP. The
script was removed by the final build; no instrumentation was added to the page.

| Stage | Before | After |
| --- | ---: | ---: |
| Navigation to workflow mount | 46 ms | 76 ms |
| List saved workflows | 4,624 ms | 3,090 ms |
| Load selected SQL and conversation | 5,682 ms | 4,096 ms |
| Document response to plan request | 355 ms | 23 ms |
| Plan request | 10,899 ms | 11,329 ms |
| Plan response to visible graph | 46 ms | 67 ms |
| **Navigation to visible graph** | **21,654 ms** | **18,682 ms** |

These are individual observations, not controlled benchmark medians. Remote
latency changed. The dev server also restarted during the build, clearing its
in-memory run history: the baseline run-history response was 1,436,543 bytes,
whereas the subsequent response was empty. The initial browser viewport was
480 px wide; the verification used the supported 1280 px desktop viewport.
Consequently the 2.97-second total difference cannot be attributed entirely to
these changes. The removed opening debounce and reduced polling are directly
observable in the request waterfall.

## Native breakdown

A temporary server built from the same checkout logged the stages inside
`WorkflowQuery::plan`. One representative successful request measured:

| Native stage | Duration |
| --- | ---: |
| Open connection | <1 ms |
| Execute native EXPLAIN | 10,320 ms |
| Drain its report | 4 ms |
| Decode structured report | 2 ms |

Other sampled EXPLAIN requests took 8,901–11,707 ms. The native JSON report was
2,518,784 bytes before Studio decoded and serialized its 1,317,209-byte response.

A five-second native stack sample showed the ADBC execution thread predominantly
parked inside `spiral_session::block_on`. Active samples included SQL binding,
`PlanPreResolver`, `SpiralDbCatalogClient::resolve_resource`,
`resolve_resolved_signal_plan_over_members`, `SnapshotObjectRepository::read_definitions`,
and SpFS reads. This points investigation toward asynchronous source/catalog
resolution. It does not prove which remote operation accounts for every second.
Native EXPLAIN also computes optimized and physical diagnostics; the default
Workflow display consumes the logical graph.

## Changes

- Debounce SQL edits, but start planning an opened workflow immediately.
- Key plan invalidation by workflow identity and SQL. Rename and save revision
  updates preserve the current graph and any in-flight plan.
- Fast run polling applies to active runs and run submission. Loading or saving a
  document no longer polls the full run history every 500 ms; it uses the existing
  five-second idle cadence. In the baseline, document loading fetched the same
  1.44 MB response about twenty times before the document arrived.
- Keep `loaded` false until the complete document is installed. Graph, Results,
  and Assistant status show loading feedback instead of a premature empty/error
  state.
- Log native planning phases without recording SQL, credentials, or result data.
  Enable with `RUST_LOG=spiral_robotics_backend::workflow=info`.

## Verification and remaining work

The final browser suite passed **133 tests**. Rust type checking, formatting,
`git diff --check`, and the release web build passed. Browser inspection confirmed
39 stages, accurate loading statuses, and a local rename/discard without an extra
plan request. No workflow was saved or executed for these checks. The temporary
profiling server was stopped. Existing staged edits were preserved.

Clippy stopped at the existing `suspicious_else_formatting` finding in
`crates/frontend/src/activities/rollout/workspace.rs:267`, outside this change.

The dominant native latency remains unresolved. No Studio cache, invented graph,
or Spiral workaround was added. Investigating/fixing the native catalog and
planning path is the next substantive step; permission to extend changes into
SpiralDB is pending. This was a focused performance review, not the optional
independent two-agent Impeccable design critique.
