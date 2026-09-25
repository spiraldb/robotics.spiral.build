# Studio HTTP API

The backend owns the HTTP API. Its only client is the Dioxus frontend, which has no
backend crate dependency or server-function bindings; the two share request and
response types through `crates/common` and ship together. There is no generated
schema or versioned contract.

## Request and response conventions

1. Fetch `GET /api/settings/connection`, the saved connection. Its `endpoint` and
   `project_id` name the project Studio starts with.
2. Send `X-Spiral-Endpoint: <endpoint>` and `X-Spiral-Project: <project_id>` on
   project reads and commands. Omit the project header before a project is chosen;
   Studio-only features such as robot setup still work. Studio-wide `/providers`,
   `/proxy`, `/account` reads, `GET /settings/connection`, and
   `POST /datasets/hugging-face/inspect` need neither header.
3. The backend keeps no current project. A request may name any project on the
   saved endpoint, which the backend reaches as the signed-in user. A request
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

Every route is a read or a command. A read is cancelled when its caller
disconnects; the browser's shared ten-second polling deadline aborts the fetch,
which drops the read. A cancelled query stops fetching its results, but a
statement Spiral is already executing finishes there. A command runs to
completion even if its HTTP caller disappears. A timeout or dropped connection
therefore does **not** prove that a command failed or was cancelled. Refresh
state before deciding whether to issue another command; this API does not
provide automatic mutation deduplication.

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
The frontend builds model requests for the provider `/providers` names and sends
them through `/proxy`, which authenticates them. It parses provider streams and
owns retry decisions and tool execution. Disconnecting cancels the proxied request.

## Feature notes

Paths are relative to `/api`. Each feature's routes are in its `http.rs`, and
request and response types are in `crates/common`.

### Spiral account

Studio reaches Spiral as a signed-in user, through the same WorkOS device-code
sign-in and shared session file as `spiraldb login`. The backend holds the tokens;
responses never include one.

- `GET /account` returns `SpiralAccountStatus`: `signed_out` (with an optional
  `problem`), `signing_in` (`verification_url`, `user_code`), or `signed_in`
  (`email`, `organization`).
- `POST /account/sign-in` starts a sign-in the user confirms at `verification_url`;
  it completes in the background. Starting again replaces an unconfirmed one.
- `GET /account/organizations` and `GET /account/projects` list the user's
  organizations and the projects they can open in the current one.
- `POST /account/sign-out` and `PUT /account/organization`
  (`{"organization_id": …}`) name the mounted project. They refuse while any
  project is recording, rolling out, or training, then close every open project
  and clear the saved project.

### External HTTP and credentials

`GET /providers` and `/proxy?url=…` (any method) are Studio-wide and work without
the project headers. They do not select credentials or destinations based on a project.

`StudioProviders` contains an optional `llm` (`base_url`, `default_model`). Its key
stays on the backend.

Percent-encode the upstream URL in the `url` query parameter. Use the upstream
HTTP method, content headers, and original request body, including binary or
multipart content. There is no JSON or metadata envelope. The relay attaches the
key Studio holds for the destination's origin: the LLM key for the configured
provider, and the saved Hugging Face token for `https://huggingface.co`. It
refuses requests from other sites (`Sec-Fetch-Site` other than `same-origin`, or
a foreign `Origin`), removes Studio cookies, project and browser-origin headers,
and manages HTTP framing and connection headers.

The response can contain JSON, text, binary data, or a live SSE stream. Provider
errors retain their upstream status and body; relay setup/transport failures use
Studio's structured errors. Dropping the browser request cancels transport.
Provider-specific request construction, stream parsing, error handling, and
retries belong to frontend clients. Native dataset queries and persistence keep
their existing project APIs.

### Training and rollout

Training selection and assembly of catalog/detail responses are frontend state.
Rollout IDs are execution `session_id` values, distinct from their training `run_id`.
Every rollout is persisted under its ID and survives a restart; one still running
when Studio stopped reads as failed. Stop targets an exact ID.

### Recording sessions

RecordingStart contains `dataset_id`, `leader`, and `follower`. The browser picks
the pair; the backend revalidates hardware admission before recording begins.

Take IDs refer to the current recording session. Published dataset content
uses `/datasets/episode-topics` and episode queries with explicit
dataset/member identities; no recording-header fallback exists.

### Settings

`SpiralConnectionUpdate.huggingface_token` is write-only: omitted or `null` keeps
the saved token; a non-empty string replaces it; an empty string removes it.
Settings responses contain only `huggingface_token_configured`. The backend keeps
the token in its local SQLite database and supplies it to the native ADBC
database. Saving a changed token replaces the project connection through the
normal settings lifecycle. Project exports and agent context do not contain it.

### Hugging Face import

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

### Curation

A curation is one opaque, versioned JSON payload per graph: the frontend owns
its schema and `encoding` version, and the backend stores, lists, and loads it
by identity without reading it (`GET /curations`, `GET /curations/{id}`). Save
(`POST /curations/save`) carries the revision the document was loaded at, 0
for one never saved, and returns the stored copy one revision higher; a
mismatch fails with `conflict`. A payload over 32 MiB fails with
`invalid_input`. This is the only JSON route with a raised request-body bound,
twice the payload limit so a document's JSON framing always fits.

`POST /curations/publish` takes `{name, statement}`: a view name, one plain
identifier at the catalog root, and the read-only `SELECT` or `WITH` the
browser compiled from an output. The backend refuses an invalid name or
statement with `invalid_input` and a name held by a table or Dataset with
`conflict`, then creates the view with one `CREATE OR REPLACE VIEW`.
`POST /curations/unpublish` takes `{name}` and drops the view.
Both answer `null`. See [curation](curation.md).

## Files and media

- `POST /project/import`: raw `.spiral-project` bytes with the project header. A
  body past 256 MiB fails with 413. The browser sends its native File without
  making a project-sized WebAssembly copy. Import returns `ProjectImportSummary`.
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

Each backend feature owns its HTTP adapter, `http.rs`, beside the code it calls:
a manager for features with runtime state (training, rollout, curation), or
an operations type over the request's
`ProjectContext` (`robots/operations.rs`, `recording/take.rs`,
`recording/catalog.rs`). Handlers extract the request, call that owner, and
return its result; `OperationError` is the failure response. `Project` opens the
project a request names, before its body is read.

Each adapter returns two plain Axum routers, `reads()` and `commands()`.
`http/routes.rs` merges them and applies the one lifetime rule: commands run to
completion in their own task, reads stay in the request. `Created<T>` declares a
201 JSON response. A route that needs its own request bound sets it with Axum's
`DefaultBodyLimit` layer, as `/curations/save` and `/project/import` do.

Shared message types live in the corresponding features of `crates/common`.
Named responses stay named through application operations and UI presentation.
Router tests send ordinary HTTP requests through the real router.

The frontend's feature clients live beside their consuming features, for example
`activities/training/client.rs` and `settings/robots/client.rs`. Each owns request
construction and the query domains its mutations invalidate. `ProjectClient`
owns session identity, request/response guards, and mounted task lifetime;
`HttpRequest` owns serialization, failure decoding, and fetch cancellation. Neither
central component knows the set of feature operations.

Dioxus renders the UI and serves the application shell; it does not define or
implement the wire protocol.
