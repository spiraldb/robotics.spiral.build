# Studio HTTP API

The backend owns the HTTP contract. The Dioxus frontend is an HTTP client with no
backend crate dependency or server-function bindings. A Qt client, script, or test
uses the same requests and receives the same policy and admission checks.

`GET /api/openapi.json` provides the OpenAPI 3.1 document, including request,
response, enum, and error schemas generated from the shared Serde message types.
It works before project storage is ready. Its paths are relative to `/api`.
The document's version identifies the API contract; Rust symbol names and Dioxus
builds do not determine endpoint URLs. Breaking wire changes require an explicit
contract revision and client migration. This migration removes the former
`/api/server-functions/*` endpoints and replaces `/api/session` with the combined
`/api/project/status` read.

Version 2.0.0 replaces the settings response field `spiral.namespace` with
`spiral.table_prefix` and uses flat Studio table and import resource names.

## Request and response conventions

1. Fetch `GET /api/project/status`. The response contains `session` and `readiness`.
2. Capture `session.id`, an opaque activation identity. It differs from
   `session.project_id`, the persistent Spiral project ID.
3. Send `X-Project-Session: <session.id>` on every JSON read or command except
   project status and OpenAPI. Opening another project invalidates the old identity,
   even if a later activation returns to the same persistent project.
4. Keep that identity with the request until its response is processed. A client
   must discard a response if its current project identity changed in the meantime.

The header is a project isolation guard, not an authentication credential. Existing
listener and deployment access controls still apply.

JSON bodies use `Content-Type: application/json`. Payloads are the documented
objects themselves; there is no Dioxus envelope or outer `config`, `request`, or
`command` wrapper. Path components must be percent-encoded. GET parameters go in
the query string, never in a request body. All API responses have
`Cache-Control: no-store`.

Successful JSON calls return 200, except creating a training run, starting a
recording, and creating a robot return 201. Commands with no result return JSON
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
| `project_transition` | 409 | Project replacement is in progress |
| `stale_project` | 409 | Request belongs to an old activation |
| `incompatible_workflow` | 422 | Stored Workflow is incompatible |
| `project_unavailable` | 503 | Required project services are unavailable |
| `external` | 502 | An upstream operation failed |
| `internal` | 500 | Backend failure |

HTTP extraction failures also use `invalid_input`, with 400 for malformed JSON or
query values, 422 for a mismatched JSON shape, 413 for oversized payloads, 415 for
unsupported content types, and 405 for an unsupported method. The frontend
classifies an unreachable backend or an invalid/non-JSON response as `transport`;
it never exposes an upstream HTML error page as a product message.

## Reads, polling, and lifetimes

Commands invalidate related reads; command responses do not replace fetched
state. The browser keeps drafts, selections, and layout locally. Polling waits
until a read completes before starting its next delay, cancels obsolete reads,
slows down in background tabs and on failures, and never automatically retries a
mutation. See [frontend fetch flow](frontend-fetch-flow.md) for intervals and
state ownership.

Backend task ownership is explicit: an abandoned asynchronous read is cancelled,
and bounded backend reads time out after eight seconds. The browser's shared
polling deadline is ten seconds. An admitted command continues if its HTTP caller
disappears. A timeout or dropped connection therefore does **not** prove that a
command failed or was cancelled. Refresh state before deciding whether to issue
another command; this API does not provide automatic mutation deduplication.

Project scope is checked before JSON or upload bodies are consumed. Command
owners acquire admission again before consequential work. Project replacement
cannot redirect an old request into the new project. Uploading a project does
not hold project admission for the entire network transfer: admission is checked
again after the bounded upload completes.

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

Fetch identity, then telemetry (replace the placeholder with `session.id`):

```sh
curl http://127.0.0.1:8080/api/project/status
curl -H 'X-Project-Session: <session.id>' \
  http://127.0.0.1:8080/api/robots/telemetry
curl -H 'X-Project-Session: <session.id>' \
  'http://127.0.0.1:8080/api/recording/signals?generation=0&offset=0'
```

Submit a read-only SQL query with an explicit row bound:

```sh
curl -X POST http://127.0.0.1:8080/api/queries \
  -H 'X-Project-Session: <session.id>' \
  -H 'Content-Type: application/json' \
  --data '{"statement":"SELECT 1 AS example","max_rows":10}'
```

A recording start body is `{"dataset_id":"example"}`. A pause update is
`{"paused":true}`. A recording stop body is `{"keep":true}`. All remain subject
to backend readiness, hardware, and activity validation.

Training polling accepts `?selected=<run-id>` to include bounded full history for
one run. Omit it when only the run catalog and current metrics are needed.
Workflow and Assistant commands use the tagged enum shapes in OpenAPI; reads of
`/workflow` and `/workflow/assistant` remain the authoritative state observations.

## Endpoint inventory

All paths below are relative to `/api`. Request and response names refer to the
schemas served by `/api/openapi.json`. `—` means no request body.

### Project

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| POST | `/project/import` | Raw project bundle | ProjectImportSummary |
| GET | `/project/status` | — | ProjectStatus |
| GET | `/recording/signals` | RecordingCursor (query) | RecordingWindow |

### Training

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| POST | `/training/runs` | TrainingConfig | TrainingRun |
| POST | `/training/runs/{id}/stop` | — | `null` |
| DELETE | `/training/runs/{id}` | — | `null` |
| POST | `/rollout` | RolloutConfig | RolloutSnapshot |
| POST | `/rollout/stop` | — | `null` |
| GET | `/training` | TrainingSelection (query) | TrainingSnapshot |
| GET | `/rollout` | — | RolloutSnapshot |

### Episodes

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| GET | `/recording-datasets` | — | Vec<RecordingDatasetSummary> |
| GET | `/recording-datasets/{dataset_id}/episodes` | — | EpisodeCatalog |
| POST | `/recording` | RecordingStart | EpisodeInfo |
| POST | `/recording/stop` | RecordingFinish | EpisodeInfo |
| PATCH | `/recording` | PauseUpdate | `null` |
| DELETE | `/episodes/{id}` | — | `null` |
| GET | `/episodes/{id}` | — | EpisodeInspectionSnapshot |
| POST | `/episodes/{id}/seek` | EpisodeSeek | `null` |
| POST | `/replay` | ReplayStart | `null` |
| PATCH | `/replay` | PauseUpdate | `null` |

### Robots

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| GET | `/serial-ports` | — | Vec<SerialPortSummary> |
| POST | `/serial-ports/identify` | SerialPortScan | SerialPortIdentification |
| POST | `/cameras/refresh` | — | Vec<CameraStatus> |
| GET | `/robots/telemetry` | — | TelemetryFrame |
| GET | `/robots/setup` | — | RobotSetupSnapshot |
| GET | `/robots` | — | Vec<RobotSummary> |
| POST | `/robots` | NewRobot | RobotSummary |
| POST | `/robots/import` | Robot configuration JSON object | RobotSummary |
| GET | `/robots/{id}` | — | RobotSummary |
| PATCH | `/robots` | RobotUpdate<Vec<CameraBinding>> | RobotSummary |
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

### Settings

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| GET | `/settings/connection` | — | SpiralSettings |
| PUT | `/settings/connection` | SpiralConnectionUpdate | `null` |
| GET | `/settings` | — | DataSettings |
| PUT | `/settings` | SettingsUpdate | DataSettings |
| POST | `/settings/spiral/test` | — | ConnectionTest |
| POST | `/settings/training/test` | — | ConnectionTest |

### Queries

The recording catalog uses `/recording-datasets`; `/datasets` lists Spiral dataset
resources. These return distinct `RecordingDatasetSummary` and `DatasetSummary` contracts.

| Method | Path | Body or query | Response |
| --- | --- | --- | --- |
| GET | `/data-catalog` | — | ProjectResource[] (actual namespace paths and resource kinds) |
| GET | `/datasets` | — | DatasetSummary[] |
| POST | `/datasets/inspect` | DatasetSelection | DatasetInspection |
| POST | `/datasets/episode-topic` | EpisodeTopicRequest (dataset, topic, episode_id) | SpiralQuerySnapshot (up to 100 messages from the exact recording ID) |
| POST | `/datasets/episode-video` | EpisodeTopicRequest (dataset, topic, episode_id) | EpisodeVideo (H.264 codec and timestamped Annex B packets; up to 18,000 packets / 64 MiB encoded) |
| GET | `/relations` | — | RelationCatalog |
| POST | `/queries` | QueryRequest | SpiralQuerySnapshot |
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
| GET | `/workflow` | — | WorkflowSnapshot |
| POST | `/workflow/commands` | WorkflowCommand | WorkflowSnapshot |
| GET | `/workflow/assistant` | — | WorkflowAgentSnapshot |
| POST | `/workflow/assistant/commands` | WorkflowAgentCommand | WorkflowAgentSnapshot |

## Files and media

- `POST /project/import`: raw `.spiral-project` bytes with
  `Content-Type: application/vnd.spiral-robotics.project` and the project header.
  The backend enforces the 256 MiB limit against both the advertised length and
  actual streamed bytes. The browser sends its native File without making a
  project-sized WebAssembly copy. Import returns `ProjectImportSummary`.
- `GET /project/export?session_id=...`: project bundle download.
- `POST /robots/import`: the robot configuration JSON object, in the same format
  as the existing portable robot file.
- `GET /cameras/stream.mjpeg?id=...`: backend-global camera stream. Camera IDs may
  contain device-path separators and must be percent-encoded.
- `GET /episodes/{id}/videos/{stream}?session_id=...&segment=0`: MP4 video; supports
  a single HTTP byte range with 206 and `Content-Range`. An unsatisfiable range
  returns 416. Media/download project identity stays in the URL so browser-native
  image, video, and download elements can request it.

Existing media/download error responses can contain plain text; JSON commands,
reads, and uploads consistently use the structured failure above. Camera/media
streaming and backend provider SSE are separate from browser state polling.
There is no browser event stream or `/api/events` endpoint.

## Maintaining the boundary

Each backend feature owns its application operations and HTTP adapter together:
`training/{commands,http}.rs`, `rollout/{commands,http}.rs`,
`robots/{commands,http}.rs`, and the corresponding recording, data, workflow, and
project-session modules. `http/routes.rs` only composes these features.

The dependency direction is delivery → application → project resources. Application
operations take a transport-neutral `ProjectContext`; they never import Axum or
HTTP state. The context retains one project identity and the backend task executor.
`project_session/tasks.rs` owns cancellation versus command completion,
`project_session/setup.rs` builds the setup projection, and
`recording/supervisor.rs` finalizes automatically stopped recordings.

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
