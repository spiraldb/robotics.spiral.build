# Workflow activity: agentic data-enrichment workbench (PoC)

## Context

The Studio's Data activity answers questions the data already has columns for. Semantic questions — "show me all the moments where X happened" — need features that live only in camera pixels. This plan adds a **Workflow activity** (peer of Live/Data/Training/Rollout): a node-based workbench where a workflow (1) reads one or more tables already materialized in Spiral, (2) runs safetensors vision models over sampled frames via **candle in the Studio backend** to materialize derived Spiral relations, (3) queries those relations with Spiral SQL, and (4) shows resulting moments. An **agent chat** (vendor-neutral LLM tool-use loop, backend-owned) decomposes a natural-language question into that workflow, runs it, and iterates. PoC goal: one viable end-to-end slice with live node states on the canvas.

User decision: Workflow is relation-first. Hugging Face datasets, MCAP files, and other external sources are imported outside Workflow; those import producers create Spiral relations that become Workflow Table roots. One imported MCAP may therefore appear as multiple tables on the canvas. External URLs, repositories, files, and import controls must never become Workflow nodes. Selected models now are SigLIP zero-shot (YOLO stretch), while the architecture still permits a model chosen by the agent.

## Load-bearing research facts

**Frontend** (Dioxus 0.7.10): adding an activity is a known mechanical 16-point edit list ([activity.rs](crates/spiral-robotics-studio/src/frontend/app/activity.rs), [routes.rs](crates/spiral-robotics-studio/src/frontend/app/routes.rs), [shell.rs](crates/spiral-robotics-studio/src/frontend/app/shell.rs), icons, notifications, workspace guards, tailwind.css, spec/). [TrainingWorkspace](crates/spiral-robotics-studio/src/frontend/screens/training/workspace.rs) is the template for an activity owning its own `PanelWorkspace` (no Timeline). Controllers = Copy structs of Signals hoisted in `StudioShell`. One SSE stream `/api/events` (named events, reconnect loop) carries all job progress. Query media previews (`semantic_type`, inline image cells, video previews) landed in commit `29d2c7a` and are the rendering path Results should reuse when execution arrives.

**dioxus-flow** (user's crate, git dep, pin a rev): dioxus 0.7.10 exact match; pointer-events (no conflict with panels' HTML5 drag-docking); signals-owned nodes/edges; custom `node_view` (Tailwind-friendly) + `Handle`s; `on_connect`/`is_valid_connection`/`on_delete`; `FlowHandle::auto_layout(LeftToRight)+fit_view` (animated — the "agent builds the graph" moment); `--df-*` CSS vars mapped to `--tone-*`; exports `STYLESHEET`.

**Backend**: axum 0.8; one ADBC pool (4 conns); [StorageManager](crates/spiral-robotics-studio-backend/src/backend/storage/manager.rs): `ingest_batches` (CreateAppend, auto-creates), `replace_batches` (atomic whole-relation swap), `execute_update_sql`, `run_query_with_byte_limit` (1000-row/4MiB caps). **`write_batches` silently no-ops on an empty vec** (re-run hazard, handled below). `/api/spiral/query` is read-only-gated (SELECT/WITH + mutation blocklist in [query.rs](crates/spiral-robotics-core/src/query.rs)) → all workflow mutations are backend-constructed structural SQL (precedent [sql.rs](crates/spiral-robotics-studio-backend/src/backend/storage/sql.rs)). [TrainingManager](crates/spiral-robotics-studio-backend/src/backend/training/manager.rs) is the job pattern (Arc<Inner> + Mutex<Snapshot> + broadcast → SSE + AtomicBool cancels + snapshot_json persistence). [CatalogMutationLease](crates/spiral-robotics-studio-application/src/catalog_lease.rs) is a **single global exclusive slot**. candle-core 0.11 + tokenizers 0.22 already compiled transitively; RolloutManager is the backend-local safetensors-inference precedent.

**SpiralDB import producers** (sibling checkout; these capabilities populate Spiral before a workflow reads the resulting relations; they are not Workflow node kinds):
- **HF**: `spql.parquet.read('hf://datasets/{repo}@{rev}/data/**/*.parquet')` works (globbing, range reads, `HF_TOKEN` env). `spql.blob.external(uri, mime_type=>'video/mp4')` = zero-copy remote references; `spql.mp4.dataset` over a staged blob table → `{ds}.assets` (media + video_index; `index_tier='dependency'` = sparse H.264 random access — exactly the frame-sampling workload). No SQL directory listing → backend constructs mp4 URI lists from LeRobot `meta/info.json` (reqwest).
- **Frames**: correlated comma generator `FROM {ds}.assets a, spql.video.frames(a.media, spql.video.frame_indices(ARRAY[…])) f` with engine-side `spql.video.decode` + `spql.image.resize/to_tensor` → Arrow over ADBC (the proven training path; scoped per episode/stream/segment).
- **MCAP**: `CREATE DATASET … USING spql.mcap.dataset FROM staged(key, blob)` → structure relations (files/topics/channels/messages/chunks + per-topic **raw payload** tables). **No message decoding exists** (no CDR/ROS/protobuf; the `convention` option is dead code) → MCAP camera frames require studio-side payload decoding (protobuf/foxglove `CompressedImage` → JPEG first; CDR later).
- **Everything is synchronous** — statements return one integer; no progress, no background jobs. The Studio owns orchestration and progress.
- `spql.ai.embed` = CLIP B/32 image-only behind a **closed allow-list by design** (plan 057 non-goal: arbitrary HF execution) — confirms the Studio backend is the right home for open-ended models. **No text embedding exists** engine-side.
- Vector (fast-follow material): `CREATE VECTOR INDEX … (metric='cosine')` + `spql.search.vector('idx', ARRAY[literal], top_k=>N)`; brute-force `spql.tensor.cosine_similarity` needs no index.
- Docs: use the generated `reference/` pages (implemented behavior); `resources/vector-index.mdx` and the `CREATE DATASET … AS SELECT` example in video.mdx are stale/wrong.

**LLM integration is vendor-neutral** (user requirement): the agent loop speaks the OpenAI-compatible chat-completions + tool-calling wire format over reqwest against a configurable endpoint (default OpenRouter, `https://openrouter.ai/api/v1`). `LLM_BASE_URL` / `LLM_API_KEY` / `LLM_MODEL` are backend env config (secrets are backend-scoped per PRODUCT.md); any OpenAI-compatible provider or local server works.

**Design routing (impeccable)**: precise extension of the incumbent one-light world (like Rollout — no concept roll). New component family (node cards/edges/canvas) inside existing tokens; run states obey Exceptional-Status + Spent-Colour rules; DESIGN.md + spec/ updated after implementation (AGENTS.md).

## Architecture (design-review verdicts folded in)

### Nodes (4 kinds; edges = lineage)

1. **Table** — a qualified Spiral relation name plus the fields needed by the next node. Multiple Table roots are valid: for example, `pick_place.camera_frames` and `pick_place.joint_state` can originate from one imported MCAP. Import provenance remains in Data and is not encoded as an external-source node. Frame-addressing conventions are normalized when relations are materialized so Model executors do not fork by import origin.
2. **Model** — `selector` (registry id `siglip-base` | `yolov8n`; shape reserves `hf_repo+revision`), config (SigLIP: `concepts`; YOLO: `classes`). **Output relation name is server-derived** from (workflow_id, node_id) — never agent/user-chosen. Concept slugs sanitized + deduped; the resolved concept→column map is echoed to UI and agent.
3. **Query** — SQL with **`{{node:<id>}}` placeholders** for upstream relations; backend substitutes server-derived names (no SQL parsing, no identifier injection surface). Read-only-gated. Residual, accepted for a single-user studio: agent SQL can read any relation in the catalog.
4. **Result** — sink; renders rows/moments via the existing query media-preview rendering (thumbnails day one; a query can select a decoded `spql.video.frame_at` thumbnail per moment).

Validation: one or more Table roots, Models and Queries read only upstream relation outputs, Query references only upstream node placeholders, and Result is terminal. Allowed lineage transitions are Table→Model, Table→Query, Model→Query, and Query→Result.

### Feature storage & re-runs

- **One wide relation per Model node** (`(episode_id, stream_id, frame_index, episode_time_ns, score_<slug>…)`): conjunctive moment queries need one scan, no pivots/self-joins (the SQL class LLMs botch). Config change = full recompute (output is a pure function of input relations + model + config).
- **Run start**: write one zero-row batch of the planned schema via `replace_batches` — atomically clears prior data, creates on first run, locks schema before chunk 1 (also works around the empty-batch no-op that would otherwise leave stale rows). Then `ingest_batches` per chunk.
- **Skip-if-unchanged**: fingerprint (model+revision+config+input-relation identity+sampling) → `Complete{reused}` without recompute.
- **Edits mark downstream `Stale`** (data kept, labeled); drop relations only on node/workflow delete. **One active run per workflow** (reject concurrent run).

### Execution (WorkflowManager, TrainingManager pattern)

- Validate → topo order → **sequential node execution** (one Metal context; pool headroom for interactive queries/recording; legible progress). Within a Model node, **pipeline** engine decode of chunk N+1 with candle forward of chunk N (bounded channel, depth 1–2).
- Manifests per (episode, stream, segment) — the frame-read SQL is scoped to that — split by **memory budget** (frames/chunk = budget ÷ H×W×3×4; ~64 at 384², ~256 at 224²).
- `WorkflowRunSnapshot { run_id, phase, node_states: Pending|Loading|Running{done,total}|Complete{rows,elapsed,warnings}|Failed{reason}|Stale|Cancelling }` → new SSE kinds `workflow_snapshot`/`workflow_update`, throttled ~4Hz. Persisted to new `workflows`/`workflow_runs` relations **on phase transitions and terminal states only** (delete-then-insert per run_id).
- **Failures**: quarantine-and-continue per (episode, segment) on decode/read faults (truncated MP4s are routine); fail node on model-load/write/schema errors, cancellation, or >50% quarantined. Snapshot carries a bounded quarantine summary (counts + first ~20 reasons); `Complete-with-warnings` is distinct from clean success and flows into agent tool results ("2 of 40 episodes unreadable").
- **Lease boundary**: import DDL (staged-table creation and `CREATE DATASET`) belongs to Data's ingestion path, not `WorkflowManager`. Workflow execution starts only after its input relations exist. Feature-relation writes and snapshot persistence hold no import lease (CAS-retry precedent: training snapshots), so model/query phases operate over existing Spiral relations.
- **ModelRunner**: hf-hub download (cached across runs; visible `Loading` sub-state — first load is 10s+), candle 0.11 (workspace lockstep), Metal-first/CPU-fallback, model owned by a dedicated worker thread (RolloutManager/spiral-ai precedent), model + Metal buffers dropped at run end. SigLIP: text tower once per run over concepts; image tower per chunk; sigmoid(img·text) → score columns.

### Agent (backend tool-use loop, vendor-neutral)

- Manual loop over `POST {LLM_BASE_URL}/chat/completions` (OpenAI-compatible; OpenRouter default): tools declared as function-calling JSON schemas, loop while the response carries `tool_calls`, results appended as `role:"tool"` messages; provider/model swappable via env. Session per workflow. Tools: `list_relations`, `describe_relations` (names/columns/types/row-counts only), `list_model_capabilities`, `set_workflow(spec)`, `run_workflow`, `wait_for_run(run_id, timeout≤60s)` (returns compact delta: phase, per-node progress, quarantine count, done), `run_query(sql, max_rows≤50)` with an agent-specific serializer (≤2KB/cell, ~20KB total, typed placeholders for binary/image/tensor cells, "N of M rows" + schema header).
- `set_workflow` **validates-and-rejects** with machine-readable node-id-addressed errors (unknown model, cycle, non-read-only SQL, non-upstream placeholder); soft warnings (e.g. frame-count estimate) ride the success payload, which **echoes resolved relation names and planned score columns** so Query SQL is written against ground truth.
- **Long runs**: ETA (from planned frame count) under ~2 min → chained `wait_for_run`; over → agent ends its turn after kickoff ("running — watch the canvas") and the backend **auto-resumes the session with a synthetic completion message** when the run finishes. Iteration cap per turn; backoff on 429/overloaded; keys never echoed into tool results/snapshots/logs.
- Turn transport: `POST /api/workflow/agent/turn` → 202; agent text + tool-call summaries stream over a new SSE kind `workflow_agent` into the chat.
- System prompt: data-model crib sheet, node config schemas + placeholder convention, curated spql reference (implemented pages only).
- Security property: **the agent never authors mutating SQL** — mutations derive structurally from the validated spec; agent free-text SQL passes the read-only gate.

### UI (Workflow activity)

Four panels in an activity-owned PanelWorkspace (`workflow-relation-canvas-v1`): **Assistant** + **Node** tabbed left at 30%; **Workflow** canvas dominant upper-right; **Results** lower-right. Stage 1 ships a truthful static scaffold: Assistant and Results name that their transports are not connected, Fit is the only active canvas action, and selecting or keyboard-activating a node opens its Node inspector without auto-switching away from Assistant on first render. Node cards use the bench vocabulary: paper, hairline, 4px radius, 12px title + 10–11px detail lines; running/failure/progress treatments remain future execution states. Canvas uses a subtle dot grid on `--tone-viewport`, maps `--df-*` to tone tokens, and omits the MiniMap. The example graph has two Table roots (`pick_place.camera_frames`, `pick_place.joint_state`), a SigLIP Model that writes server-derived `workflow.siglip_scores`, one Query, and one Result.

## Milestones

0. **Day-1 spike (parallel to M1)**: candle-transformers SigLIP image+text towers + tokenizer on Metal, CPU-fallback timing. Highest schedule risk — Metal kernel coverage and throughput set demo pacing. Query media-preview rendering is already available from commit `29d2c7a`.
1. **Canvas scaffold (implemented)** — fifth activity registered beside Data and before Training; dioxus-flow pinned/themed; relation-first five-node example; selection + read-only Node inspector; `WorkflowController` hoisted; four-panel desktop and 760px mobile-switcher behavior verified. No backend execution, assistant turns, or result rows are implied.
2. **Engine, SQL-only** — **freeze full WorkflowSpec contract + capabilities endpoint** (agent tool schemas mirror it); WorkflowManager + SSE + persistence/reload + cancel; Table→Query(`{{node:…}}`)→Result end-to-end over existing Spiral relations. A visual query builder with live states; all plumbing proven.
3. **Model node** — ModelRunner (hf-hub, SigLIP zero-shot, Metal), manifests + budget chunks + pipelined decode/forward, zero-row replace + chunked ingest, quarantine summaries, live progress; manual end-to-end demo on a studio dataset. Measure in-engine ffmpeg decode throughput week 1 (it drives chunking/ETA math).
4. **Agent chat** — vendor-neutral LLM loop (OpenAI-compatible tool calling) + tools + validate/reject + long-run lifecycle; chat panel with streamed turns; agent authors/runs workflows from a question and iterates on SQL.
5. **External ingestion in Data** — HF LeRobot import end-to-end plus MCAP import (**structure = deliverable**; protobuf CompressedImage frames = **stretch**). Each producer materializes one or more named Spiral relations; Workflow discovers those relations as Table roots but never exposes an HF/MCAP Source node or import control. LeRobot layout variance (v2/v2.1/v3) stays isolated in the ingestion module. Stretch: YOLO Model node, moment→Data deep link, `spql.ai.embed`+vector-index Embed node.

Each UI milestone: craft-floor loaded before edits; detector once per MANUAL_DETECTOR_REQUIRED (it skips .rs — tailwind.css is its real target); finish-reviewer + documenter close the visual work; spec/ + DESIGN.md updated after implementation.

## Verification

- Backend: unit tests for DAG/placeholder/identifier validation and read-only gating; integration test running a SQL-only workflow on a seeded project; re-run test proving stale rows cannot survive (zero-row replace); architecture tests still green.
- Model path: golden test on a tiny fixture (frames → score columns land, progress + quarantine events fire); Metal vs CPU timing recorded in the spike.
- Agent: tool-schema round-trip with a mocked OpenAI-compatible endpoint; live smoke behind the env key (any provider).
- UI: dev-server visual pass early (memory: no headless browser on this Mac; do the visual pass before the pane dies; build/verify commands run unsandboxed); interaction-language test green; 760px mobile switcher check.
- Demo script: import an HF LeRobot dataset in Data → inspect the resulting Spiral relations → ask a question in Workflow → agent builds a Table-rooted graph → run lights up nodes → moments table with thumbnails.

## Risks (ranked by the design review)

1. candle SigLIP on Metal (spiked day 1; CPU fallback is 10–20× slower and changes pacing).
2. Agent turn lifecycle for multi-minute runs (kickoff-and-auto-resume specified above).
3. LeRobot layout variance across versions (isolated module, one known-good repo first).
4. In-engine ffmpeg decode throughput (measured in M3 week 1).
5. MCAP vision gap is engine-truth (no decoding) — structure queries always work; frames are stretch via studio-side protobuf decode.
