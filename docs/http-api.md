# Studio HTTP API

The backend owns the HTTP contract. The Dioxus frontend is an HTTP client with no
backend crate dependency or server-function bindings. A Qt client, script, or test
uses the same requests and receives the same policy checks.

`GET /api/openapi.json` provides the OpenAPI 3.1 document, including request,
response, enum, and error schemas generated from the shared Serde message types.
It works before project storage is ready. Its paths are relative to `/api`.
The document's version identifies the API contract; Rust symbol names and Dioxus
builds do not determine endpoint URLs. Breaking wire changes require an explicit
contract revision and client migration. This migration removes the former
`/api/server-functions/*` endpoints.

Version 5.0.0 removes robot replay and physical seek (`POST`/`PATCH /replay`,
`POST /recording/takes/{id}/seek`, the `replay` mode and telemetry field), the
robot-discovery context, stream-inspection, and save endpoints (replaced by
`POST /datasets/robot/save`), `/datasets/episode-messages` (replaced by
`POST /queries/episode/arrow`), and `/workflows/plan` and `/models/capabilities`
(both are `/queries` reads). `TopicRevision` no longer carries a dataset version.

Version 4.0.0 removes `/project/status`, the `X-Project-Session` header, and the
`project_transition` and `project_unavailable` failure codes. The backend no
longer has a current project: each request names its project with
`X-Spiral-Endpoint` and `X-Spiral-Project`, and `GET /settings/connection` names
the project a client starts on.

Version 3.0.0 replaces `/models/status`, `/models/complete`, `/models/decide`, and
`/datasets/robot-discovery/read-description` with Studio-wide `/credentials` and
`/proxy`. Provider requests and responses are handled by the frontend. The
version 2 settings field `spiral.table_prefix` and flat Studio resource names
remain in use.

## Request and response conventions

1. Fetch `GET /api/settings/connection`, the saved connection. Its `endpoint` and
   `project_id` name the project Studio starts with.
2. Send `X-Spiral-Endpoint: <endpoint>` and `X-Spiral-Project: <project_id>` on
   project reads and commands. Omit the project header before a project is chosen;
   Studio-only features such as robot setup still work. Studio-wide `/credentials`
   and `/proxy`, `GET /settings/connection`, and OpenAPI need neither header.
3. The backend keeps no current project. A request may name any project on the
   saved endpoint, which the backend reaches with the saved credentials. A request
   naming another endpoint fails with `stale_project`; read the saved connection
   again.
4. The client decides which project it works on. `PUT /settings` and
   `PUT /settings/connection` validate and save a new project; later requests
   name it. Work already running in another project keeps running there.

The headers name a project; they are not an authentication credential. Existing
listener and deployment access controls still apply.

JSON bodies use `Content-Type: application/json`. Payloads are the documented
objects themselves; there is no Dioxus envelope or outer `config`, `request`, or
`command` wrapper. Path components must be percent-encoded. GET parameters go in
the query string, never in a request body. All API responses have
`Cache-Control: no-store`.

Successful JSON calls return 200, except creating a training run and starting a
recording return 201. Commands with no result return JSON
`null`. A successful start/stop request describes acceptance or the state reached
by that command; clients fetch state to observe background work reaching its
terminal phase. Jobs are not kept alive by the polling connection.

JSON endpoint failures have this shape:

```json
{"code":"stale_project","message":"The project changed before this request completed."}
```

| Code | Usual HTTP status | Meaning |
| --- | --- | --- |
| `invalid_input` | 400 | Invalid parameters |
| `not_found` | 404 | Missing resource or endpoint |
| `conflict` | 409 | Operation conflicts with current activity |
| `stale_project` | 409 | Request names an endpoint Studio no longer uses |
| `incompatible_workflow` | 422 | Stored Workflow is incompatible |
| `external` | 502 | An upstream operation failed |
| `internal` | 500 | Backend failure |

HTTP extraction failures also use `invalid_input`, with 400 for malformed JSON or
query values, 422 for a mismatched JSON shape, 413 for oversized payloads, 415 for
unsupported content types, and 405 for an unsupported method. The frontend
classifies an unreachable backend or an invalid/non-JSON response as `transport`;
it never exposes an upstream HTML error page as a product message.
The transparent `/proxy` endpoint instead preserves the upstream HTTP status and
response body; the frontend's provider client interprets that response.

## Reads, polling, and lifetimes

Commands invalidate related reads; command responses do not replace fetched
state. The browser keeps drafts, selections, and layout locally. Polling waits
until a read completes before starting its next delay, cancels obsolete reads,
slows down in background tabs and on failures, and never automatically retries a
mutation. See [frontend fetch flow](frontend-fetch-flow.md) for intervals and
state ownership.

Backend task ownership is explicit: an abandoned asynchronous read is cancelled,
and bounded backend reads time out after eight seconds. The browser's shared
polling deadline is ten seconds. A command continues if its HTTP caller
disappears. A timeout or dropped connection therefore does **not** prove that a
command failed or was cancelled. Refresh state before deciding whether to issue
another command; this API does not provide automatic mutation deduplication.

The named project is opened before JSON or upload bodies are consumed. A request
keeps that project until its work completes; saving another project never
redirects it.

The hardware loop and safety remain backend-owned. Telemetry is a sampled display
snapshot, not the robot control clock. Its `sequence` advances with hardware
publication; repeated HTTP success does not make an unchanged sample fresh.
Recording signals retain their own samples. Read `/recording/signals` with
`generation` and `offset` (initially both zero). Responses contain at most 1,000
samples. The next offset is the returned offset plus the number of returned
`times`; adopt the returned generation. A generation change or offset gap means
replace the local window. Fetch again to catch up; the retained live window is
bounded to approximately thirty minutes at 20 Hz.

## Examples

Read the saved connection, then telemetry for its project:

```sh
curl http://127.0.0.1:8080/api/settings/connection
curl -H 'X-Spiral-Endpoint: https://api.spiraldb.dev' -H 'X-Spiral-Project: <project_id>' \
  http://127.0.0.1:8080/api/robots/telemetry
curl -H 'X-Spiral-Endpoint: https://api.spiraldb.dev' -H 'X-Spiral-Project: <project_id>' \
  'http://127.0.0.1:8080/api/recording/signals?generation=0&offset=0'
```

Submit a read-only SQL query with an explicit row bound:

```sh
curl -X POST http://127.0.0.1:8080/api/queries \
  -H 'X-Spiral-Endpoint: https://api.spiraldb.dev' -H 'X-Spiral-Project: <project_id>' \
  -H 'Content-Type: application/json' \
  --data '{"statement":"SELECT 1 AS example","max_rows":10}'
```

A recording start body is `{"dataset_id":"example"}`. A pause update is
`{"paused":true}`. A recording stop body is `{"keep":true}`. All remain subject
to backend hardware and activity validation.

Training polling accepts `?selected=<run-id>` to include bounded full history for
one run. Omit it when only the run catalog and current metrics are needed.
Workflow authoring is local to the browser. Save sends a `WorkflowDocument` whose
document revision is the expected saved version. Its optional `planned_sql` field
contains `{sql, plan}`: the exact SQL and the plan used for display. Save/load preserves this snapshot alongside SQL and conversation; a plan
for different SQL is rejected. Documents without a snapshot remain supported.
Run sends a complete `WorkflowSpec`
and returns a `WorkflowRun` containing that exact SQL definition and a run snapshot.
`WorkflowSpec` contains `id`, `name`, `revision`, and `sql`. The browser plans
by sending `EXPLAIN (FORMAT JSON) <sql>` to `/queries` and reading the native JSON
report (EXPLAIN schema version 2) from the single result cell.
The report contains logical, optimized logical, and physical diagnostic graphs,
plus the native report metadata. Logical graphs remain available if physical
lowering fails. Studio displays these with Dioxus Flow. The installed native AI
function catalog is likewise a `/queries` read of `spql.meta.scalar_functions()`.
Read executions by their run IDs. Workflow cancellation is not exposed.
The frontend builds authenticated model requests using Studio-wide `/credentials`
and sends them through `/proxy`. It parses provider streams, owns retry decisions,
conversation history, and tool execution. Disconnecting cancels the proxied request.

## Endpoint inventory

All paths below are relative to `/api`. Request and response names refer to the
schemas served by `/api/openapi.json`. `—` means no request body.

### External HTTP and credentials

These endpoints are Studio-wide and work without the project headers. They do not select credentials or destinations based on a project.

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| GET | `/credentials` | — | StudioCredentials |
| GET, POST, PUT, PATCH, DELETE, HEAD, OPTIONS | `/proxy?url=…` | Upstream method, headers, and raw body | Upstream HTTP status, relevant headers, and streamed body |

`StudioCredentials` contains optional `llm` (`base_url`, `api_key`, `default_model`),
`spiral` (`endpoint`, `spfs_endpoint`, `client_id`, `client_secret`), and
`hugging_face` (`token`) entries. These are known configured Studio credentials,
not arbitrary environment variables. The frontend uses them in memory and builds
the upstream authorization headers.

Percent-encode the upstream URL in the `url` query parameter. Use the upstream
HTTP method, authorization and content headers, and original request body,
including binary or multipart content. There is no JSON or metadata envelope.
The relay removes Studio cookies, project and browser-origin headers,
and manages HTTP framing and connection headers.

The response can contain JSON, text, binary data, or a live SSE stream. Provider
errors retain their upstream status and body; relay setup/transport failures use
Studio's structured errors. Dropping the browser request cancels transport.
Provider-specific request construction, stream parsing, error handling, and
retries belong to frontend clients. Native dataset queries and persistence keep
their existing project APIs.

### Project

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| POST | `/project/import` | Raw project bundle | ProjectImportSummary |
| GET | `/recording/signals` | RecordingCursor (query) | RecordingWindow |

### Training

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| POST | `/training/runs` | TrainingConfig | TrainingRun |
| POST | `/training/runs/{id}/stop` | — | `null` |
| DELETE | `/training/runs/{id}` | — | `null` |
| POST | `/rollout-runs` | RolloutConfig | RolloutSnapshot |
| POST | `/rollout-runs/{id}/stop` | — | `null` |
| GET | `/training/runs` | — | TrainingSnapshot (bounded catalog) |
| GET | `/training/runs/{id}` | — | TrainingRun (full bounded history) |
| GET | `/rollout-runs` | — | Vec<RolloutSnapshot> |
| GET | `/rollout-runs/{id}` | — | RolloutSnapshot |

Training selection and assembly of catalog/detail responses are frontend state.
Rollout IDs are execution `session_id` values, distinct from their training `run_id`.
Every rollout is persisted under its ID and survives a restart; one still running
when Studio stopped reads as failed. Stop targets an exact ID.

### Recording sessions

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| POST | `/recording` | RecordingStart | EpisodeInfo |
| POST | `/recording/stop` | RecordingFinish | EpisodeInfo |
| PATCH | `/recording` | PauseUpdate | `null` |
| DELETE | `/recording/takes/{id}` | — | `null` |
| GET | `/recording/takes/{id}` | — | EpisodeInspectionSnapshot |
| POST | `/recording/takes/{id}/camera` | RecordingCameraRequest | EpisodeVideo |

Take IDs above refer to the current recording session. Published dataset content
uses `/datasets/episode-topics` and episode queries with explicit
dataset/member identities; no recording-header fallback exists.

### Robots

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| GET | `/serial-ports` | — | Vec<SerialPortSummary> |
| POST | `/cameras/refresh` | — | Vec<CameraStatus> |
| GET | `/robots/telemetry` | — | TelemetryFrame |
| GET | `/hardware/status` | — | HardwareStatus (mode, cameras, issues) |
| GET | `/robots` | — | Vec<RobotSummary> |
| POST | `/robots/save` | RobotDocumentSave | RobotSummary (includes complete document) |
| GET | `/robots/{id}` | — | RobotSummary |
| DELETE | `/robots/{id}` | — | `null` |
| POST | `/robots/{id}/connect` | — | `null` |
| POST | `/robots/{id}/disconnect` | — | `null` |
| POST | `/robots/{id}/hold` | — | `null` |
| POST | `/robots/{id}/release` | — | `null` |
| POST | `/robots/{id}/goal` | RobotGoal | `null` |
| POST | `/robots/{id}/calibration/start` | — | `null` |
| POST | `/robots/{id}/calibration/finish` | — | RobotSummary |
| POST | `/robots/{id}/calibration/cancel` | — | `null` |
| POST | `/robots/{id}/calibration/reapply` | — | CalibrationApplication |
| PUT | `/mode` | ModeSelection | `null` |

RecordingStart contains `dataset_id`, `leader`, and `follower`. The browser picks
the pair; the backend revalidates hardware admission before recording begins.

### Settings

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| GET | `/settings/connection` | — | SpiralSettings (Studio-wide, no project headers) |
| PUT | `/settings/connection` | SpiralConnectionUpdate | SpiralSettings |
| GET | `/settings` | — | DataSettings |
| PUT | `/settings` | SettingsUpdate | DataSettings |
| POST | `/settings/spiral/test` | — | ConnectionTest |
| POST | `/settings/training/test` | — | ConnectionTest |

`SpiralConnectionUpdate.huggingface_token` is write-only: omitted or `null` keeps
the saved token; a non-empty string replaces it; an empty string removes it.
Settings responses contain only `huggingface_token_configured`. The backend keeps
the token in its local SQLite database and supplies it to the native ADBC
database. Saving a changed token replaces the project connection through the
normal settings lifecycle. Project exports and agent context do not contain it.

### Queries

The recording catalog uses `/recording-datasets`; `/datasets` lists Spiral dataset
resources. These return distinct `RecordingDatasetSummary` and `DatasetSummary` contracts.

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| GET | `/data-catalog` | — | ProjectResource[] (actual namespace paths and resource kinds) |
| GET | `/datasets` | — | DatasetSummary[] |
| POST | `/datasets/inspect` | DatasetSelection | DatasetInspection |
| POST | `/datasets/robot` | DatasetSelection | Saved DatasetRobot or `null`; read-only, with no model analysis |
| POST | `/datasets/robot/save` | DatasetRobotSave (dataset, robot or `null`) | `null`; the browser resolves, validates, and attaches meshes first |
| POST | `/datasets/episode-topics` | EpisodeSelection (dataset, episode_id) | DatasetTopic[] (topics with messages in the exact recording, with schema revisions and counts) |
| POST | `/datasets/topic-coverage` | TopicCoverageRequest (MessageRead with a plain window, `buckets` 8–2048) | TopicCoverage (message total, first/last log time as strings, one count per bucket) |
| GET | `/relations` | — | RelationCatalog |
| POST | `/queries` | QueryRequest | SpiralQuerySnapshot |
| POST | `/queries/episode` | EpisodeQueryRequest (episode selection, statement with `$1` bound to the episode UUID, max_rows) | SpiralQuerySnapshot |
| POST | `/queries/episode/arrow` | EpisodeQueryRequest | `application/vnd.apache.arrow.stream`: the complete result, or an error past `max_rows` (at most 10,000) or 32 MiB; see [episode playback](episode-playback.md) |
| POST | `/datasets/hugging-face/inspect` | HuggingFaceDatasetSource | HuggingFaceDatasetInspection |
| POST | `/datasets/hugging-face/import` | HuggingFaceDatasetSource | HuggingFaceDatasetImportSummary |

Dataset import accepts public Hugging Face repositories containing MCAP files. The
request has `dataset_id`, `repository`, and optional `revision` (default `main`);
imports append rows tagged with `dataset_name` to the shared root `dataset_source`
table, create the root filtered view `dataset_source_<dataset_id>`, and derive
`dataset_<dataset_id>` from that view in one transaction. Studio application state
uses root `studio_*` tables.
There is no format selector or LeRobot import. Inspection pins a revision and
lists the dataset and source relation to create. Import uses Spiral's
`spql.mcap.dataset` producer, refreshes its population, and checks episode count
before committing. Existing targets are refused. It does not publish rows into a
separate Studio dataset registry. Project snapshot restore is a separate operation.

### Workflow

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| GET | `/workflows` | — | WorkflowSummary[] |
| GET | `/workflows/{id}` | — | WorkflowDocument |
| POST | `/workflows/save` | WorkflowDocument | WorkflowDocument (new saved revision) |
| POST | `/workflow-runs` | WorkflowSpec | WorkflowRun (completed or failed) |
| POST | `/datasets/cameras` | DatasetSelection | Dataset and camera inspection |

### Curation

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| GET | `/curations` | — | CurationSummary[] |
| GET | `/curations/{id}` | — | CurationSpec |
| POST | `/curations/save` | CurationSpec (stored revision) | CurationSpec (next revision) |
| POST | `/curation-runs` | CurationSpec | CurationRun (completed or failed) |

A curation is a typed recipe over one dataset (measurements, exclusion rules,
transforms) compiled to one read-only query; see [curation](curation.md).
A run is one request that answers when the run ends, like a workflow run; the
browser keeps the run list. Save is the only write and uses optimistic revisions.

### Explorations

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| GET | `/explorations` | — | ExplorationSummary[] |
| GET | `/explorations/{id}` | — | ExplorationDocument |
| POST | `/explorations/save` | ExplorationDocument (loaded revision) | ExplorationDocument (next revision) |

An exploration is one opaque, versioned JSON payload per graph: the
frontend owns its schema and `encoding` version, and the backend stores, lists,
and loads it by identity without reading it. Save carries the revision the
document was loaded at, 0 for one never saved, and returns the stored copy one
revision higher; a mismatch fails with `conflict`. A payload over 32 MiB fails
with `invalid_input`. This is the only JSON route with a raised request-body
bound, twice the payload limit so a document's JSON framing always fits.

## Files and media

- `POST /project/import`: raw `.spiral-project` bytes with
  `Content-Type: application/vnd.spiral-robotics.project` and the project header.
  The backend enforces the 256 MiB limit against both the advertised length and
  actual streamed bytes. The browser sends its native File without making a
  project-sized WebAssembly copy. Import returns `ProjectImportSummary`.
- `GET /project/export?endpoint=...&project_id=...`: version 3 project bundle download, a ZIP
  containing exactly `studio.sqlite3` (project-scoped control records, no backend
  credentials) and `spiral.data` (MCAP sources and checkpoint artifacts).
  Import accepts only this version and also bounds total uncompressed contents
  to 256 MiB. See [project transfer](studio-sqlite.md#project-transfer).
- `POST /robots/save`: `{document, expected}`. `document` is the complete portable
  robot JSON object. `expected: null` creates a new robot; otherwise it is the
  editor baseline, compared inside the save transaction. Conflicts return 409.
  Save does not connect hardware; the frontend invokes Connect explicitly.
- `GET /cameras/stream.mjpeg?id=...`: backend-global camera stream. Camera IDs may
  contain device-path separators and must be percent-encoded.


Existing media/download error responses can contain plain text; JSON commands,
reads, and uploads consistently use the structured failure above. Camera/media
streaming and proxied provider SSE are separate from browser state polling.
There is no browser event stream or `/api/events` endpoint.

## Maintaining the boundary

Each backend feature owns its application operations and HTTP adapter together:
`training/{commands,http}.rs`, `rollout/{commands,http}.rs`,
`robots/{commands,http}.rs`, and the corresponding recording, data, workflow, and
project modules. `http/routes.rs` only composes these features.

The dependency direction is delivery → application → project resources. Application
operations take a transport-neutral `ProjectContext`; they never import Axum or
HTTP state. The context retains the project a request named and the backend task
executor. `projects/tasks.rs` owns cancellation versus command completion, and
`recording/take.rs` opens, pauses, and closes a take, including one the control
loop stops on its own.

An endpoint registers its method, path, and handler once. `http/contract.rs` derives
request/response schemas from that handler's extractors and return type, then
builds both the Axum router and OpenAPI document. `ScopedProject<Capability>`
declares the required project-bound operation owner; `Created<T>` declares a 201
JSON response. Native media handlers register their binary response description
alongside the handler. No separate global operation inventory needs updating.

Shared message schemas live in the corresponding features of `crates/common`;
the backend enables their optional `http-schema` feature. Named responses stay
named through application operations and UI presentation. Contract tests send
ordinary HTTP requests through the real router and resolve every schema reference.
Update this human-readable inventory when changing an endpoint.

The frontend's feature clients live beside their consuming features, for example
`activities/training/client.rs` and `settings/robots/client.rs`. Each owns request
construction and the query domains its mutations invalidate. `ProjectClient`
owns session identity, request/response guards, and mounted task lifetime;
`HttpRequest` owns serialization, failure decoding, and fetch cancellation. Neither
central component knows the set of feature operations.

Dioxus renders the UI and serves the application shell; it does not define or
implement the wire protocol.

Workflow Assistant execution uses the same `POST /workflow-runs` as the Run button;
the request answers when the query ends, and the browser keeps each run's exact
submitted SQL and bounded preview. The browser agent can page those previews,
attach selected native image previews to the next model request, and select a completed
run for presentation. The frontend model client sends text or standard text/image
content parts through `/proxy`; transient image messages are not allowed in persisted
conversation history.
Stopping the Assistant does not cancel an admitted native query.
