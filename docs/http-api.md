# Studio HTTP API

The backend owns the HTTP API. Its only client is the Dioxus frontend, which has no
backend crate dependency or server-function bindings; the two share request and
response types through `crates/common` and ship together. There is no generated
schema or versioned contract. Robot hardware, recording, and rollout are served
by the robot connector instead; see [robot connector API](robot-connector-api.md).

## Request and response conventions

1. Fetch `GET /api/settings/connection`, the saved connection. Its `endpoint` and
   `project_id` name the [workspace](#workspaces) Studio starts with.
2. Send `X-Spiral-Endpoint: <endpoint>` and `X-Spiral-Project: <project_id>` on
   project reads and commands. Omit the project header before a workspace is
   chosen; Studio-wide features still work. Studio-wide `/providers`,
   `/keys`, `/proxy`, `/account` reads, `/workspaces`, `/settings/connection`,
   and the Hugging Face inspections (`POST /datasets/hugging-face/inspect` and
   `POST /datasets/lerobot/inspect`) need neither header.
3. The backend keeps no current project. A request may name the project of any
   workspace on the saved endpoint, which the backend reaches as the signed-in
   user. A request naming another endpoint, or a project whose workspace was
   deleted, fails with `stale_project`; read the saved connection again.
4. The client decides which workspace it works in. `PUT /settings/connection`
   saves another one; later requests name it. Work already running in another
   project keeps running there.

The headers name a project; they are not an authentication credential. Existing
listener and deployment access controls still apply.

JSON bodies use `Content-Type: application/json`. Payloads are the documented
objects themselves; there is no Dioxus envelope or outer `config`, `request`, or
`command` wrapper. Path components must be percent-encoded. GET parameters go in
the query string, never in a request body. All API responses have
`Cache-Control: no-store`.

Successful JSON calls return 200, except creating a training run returns 201.
Commands with no result return JSON
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
| `stale_project` | 409 | Request names an endpoint Studio no longer uses, or a deleted workspace |
| `external` | 500 | An upstream operation failed |
| `internal` | 500 | Backend failure |

`external` is not a 502 because Cloudflare, in front of hosted Studio, replaces
the body of an origin 502 or 504 with its own error page; the code tells
`external` and `internal` apart.

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

## Examples

Every request carries a browser's session cookie, `studio_oauth_session`; sign in at
`http://localhost:3000` and copy it from the browser's developer tools. Read
the session's connection, then its project's training runs:

```sh
curl -b "studio_oauth_session=$STUDIO_SESSION" http://localhost:3000/api/settings/connection
curl -b "studio_oauth_session=$STUDIO_SESSION" \
  -H 'X-Spiral-Endpoint: https://api.spiraldb.dev' -H 'X-Spiral-Project: <project_id>' \
  http://localhost:3000/api/training/runs
```

Submit a read-only SQL query with an explicit row bound:

```sh
curl -X POST http://localhost:3000/api/queries -b "studio_oauth_session=$STUDIO_SESSION" \
  -H 'X-Spiral-Endpoint: https://api.spiraldb.dev' -H 'X-Spiral-Project: <project_id>' \
  -H 'Content-Type: application/json' \
  --data '{"statement":"SELECT 1 AS example","max_rows":10}'
```

`GET /training/runs` lists the runs; `GET /training/runs/{id}` adds one run's
metrics and log tail. The frontend builds model requests for the provider
`/providers` names and sends them through `/proxy`, which authenticates them. It
parses provider responses and owns retry decisions and tool execution.
Disconnecting cancels the proxied request.

## Feature notes

Paths are relative to `/api`. Each feature's routes are in its `http.rs`, and
request and response types are in `crates/common`.

### Spiral account

Every browser signs in through this environment's registered Spiral application,
with an authorization code, PKCE, and user consent. The backend keeps each session's tokens and
names the session in an HttpOnly `studio_oauth_session` cookie; responses never
include a token. Every `/api` request needs a live session and otherwise fails
with `401` and the code `unauthenticated`. Opening a project asks Spiral
whether the session's user may view it.

The sign-in pages are outside `/api`, because the browser navigates to them:

- `GET /sign-in?return_to=<path>` sends the browser to Spiral's sign-in page.
- `GET /callback` receives it back, starts the session, and returns the browser
  to `return_to`, a path on this Studio.

The account endpoints:

- `GET /account` returns the `SpiralIdentity`: `email` and `organization`.
- `GET /account/projects` lists the projects the application may open on behalf
  of this user in the consented organization.
- `POST /account/sign-out` names the mounted project. It refuses while any of the
  session's projects is training, then revokes the refresh token, closes the
  session's open projects, and ends the session and its cookie.

### Workspaces

A workspace is one Spiral project under the name Studio shows for it, with
everything Studio keeps for that project: its curations, preferences, training
runs, and robot connector. Every signed-in user sees the workspaces whose
projects Spiral lets them open; Spiral alone decides who may open, change, or
delete which. Each backend keeps its own workspaces, in its
[Studio database](studio-sqlite.md).

- `GET /workspaces` lists the `Workspace`s (`project_id`, `name`) the user can
  open, by name.
- `POST /workspaces` (`NewWorkspace`: `{"name": …, "project_id": … | null}`)
  makes a workspace on an existing project the application may open. A null
  `project_id` fails with `invalid_input`; create projects in Spiral Studio first.
  A project has at most one workspace: another fails with `conflict`.
- `DELETE /workspaces/{project_id}` forgets the workspace and every Studio
  record of its project, and closes it in every browser. The Spiral project and
  its data stay. It refuses while the project is training.
- `GET /settings/connection` returns the Spiral endpoints and the project of the
  workspace the session works in: the one it chose while that exists, else the
  oldest workspace the user can open, else empty.
- `PUT /settings/connection` (`{"project_id": …}`) makes the session work in
  that workspace and returns the same shape.
- `GET /settings` returns the named workspace's `DataSettings`: the Spiral
  endpoints and project, its `name`, and `robot_connector` address.
- `PUT /settings` (`{"name": …}`) renames the named workspace;
  `PUT /settings/robot-connector`
  (`{"address": …}`) sets the robot connector every browser in it drives. Both
  return `DataSettings` and refuse before a workspace is chosen.

### External HTTP and credentials

`GET /providers` and `GET` or `POST /proxy?url=…` are Studio-wide and work without
the project headers, though like every API request they need a session. They do not select credentials or destinations based on a project.

`StudioProviders` contains an optional `llm` (`base_url`). Its key
stays on the backend.

The provider keys are the deployment's: the backend reads them from its
environment at startup, and no endpoint reads or writes them. The Modal base
URL and token reach only the Modal service that scores progress, never the
relay or the driver.

Percent-encode the upstream URL in the `url` query parameter. Use `GET` or
`POST`, the upstream content headers, and the original request body, including
binary or multipart content. There is no JSON or metadata envelope. The relay attaches the
key Studio holds for the destination's origin: the LLM key for the configured
provider, and the Hugging Face token for `https://huggingface.co`. It
refuses requests from other sites (`Sec-Fetch-Site` other than `same-origin`, or
a foreign `Origin`), removes Studio cookies, project and browser-origin headers,
and manages HTTP framing and connection headers.

The response can contain JSON, text, binary data, or a live SSE stream. Provider
errors retain their upstream status and body; relay setup/transport failures use
Studio's structured errors. Dropping the browser request cancels transport.
Provider-specific request construction, response parsing, error handling, and
retries belong to frontend clients. Native dataset queries and persistence keep
their existing project APIs.

### Training

A training run fits a policy to a published curated view on the Modal
training service; [Training](training.md) describes the run end to end.
`POST /training/runs` (`TrainingConfig`) admits a run, records it, and starts
uploading its data; `GET /training/runs` lists every run with whether a new one
could start; `GET /training/runs/{id}` adds one run's metrics and log tail;
`POST /training/runs/{id}/stop` and `DELETE /training/runs/{id}` stop and
delete one; `GET /training/checkpoints` lists every saved checkpoint.

### Recordings

Episodes are recorded on the robot connector, which uploads each finished one to
Studio in parts ([the link](robot-connector-api.md#the-link)). Studio keeps a
whole episode (at most 256 MiB, one Spiral row) in its SQLite database until a
browser stores it: `GET /robot-connector/episodes` lists them, each
`{dataset_id, episode_id, bytes}`, and
`POST /robot-connector/episodes/{dataset}/{episode}/store` stores one as
`{episode}.mcap` in the MCAP dataset `dataset` with the browser's Spiral
access, creating the dataset with its first episode, then forgets it. Storing the
same episode again is an idempotent retry; other bytes at that episode are
refused. Rollout stores each episode as it arrives. Stored episodes are read
like any other ([datasets](#datasets)); no recording-header fallback exists.

### Datasets

A dataset is the tables under `datasets.<id>` that Studio writes itself
([datasets](datasets.md)), named by a `DatasetSelection`, `{"id": …}`.

- `GET /data-catalog` lists the project's datasets, each a `DatasetSummary`:
  its `selection`, its `format` (`mcap` or `lerobot`), and the `resource_id`
  and `version_id` of its `episodes` table, which change exactly when episodes
  are added. `POST /datasets/robot` and `POST /datasets/joint-roles` take one
  and answer what Studio keeps for that version, or `null`; their `/save`
  routes keep it.
- `POST /datasets/inspect` (`DatasetSelection`) answers a
  `DatasetInspection`: the dataset's summary, its episode count (`members`),
  and its `topics`, one per stream, each with its `name` (the topic),
  `revision` (`{stream}`, its table under `messages`), `schema`, `encoding`,
  and how many episodes (`members`) and messages (`samples`) hold it.
- `POST /datasets/episode-topics` (`EpisodeSelection`, `{dataset,
  episode_id}`) lists the streams one episode holds, each with its message
  count.
- `POST /datasets/delete` (`DatasetSelection`) drops every table under
  `datasets.<id>` in one transaction, answers and recordings included, and
  any table Studio no longer writes; a dataset with no tables is already
  deleted.

Episode content is read with read-only SQL over those tables: `POST /queries`,
and `POST /queries/episode` and `POST /queries/episodes`, which bind one
episode's number as `$1` or listed episodes' as `$1`…`$N`; each of the two has an
`/arrow` form answering one Arrow IPC stream.

Import takes a public Hugging Face repository holding MCAP files or a LeRobot
v3 dataset, as a new dataset. Downloads send the deployment's Hugging Face
token when it is set. See [datasets](datasets.md#writing).

- `POST /datasets/hugging-face/inspect` (MCAP) and
  `POST /datasets/lerobot/inspect` take a `HuggingFaceDatasetSource`:
  `dataset_id`, `repository`, and optional `revision` (default `main`). They
  pin the revision to its commit and list the files an import reads, their
  size, and the tables it creates; they change nothing. A LeRobot repository
  must hold `meta/info.json`.
- `POST /datasets/imports` (`DatasetImportRequest`, `{format, source}`)
  inspects the source again, starts an import the backend holds, and answers
  its `DatasetImportStatus` at once: `id`, the `request` with its source at
  the resolved commit, and `phase`: `listing_files`, `importing_episodes`
  with `written` and `total`, `done`, or `failed` with its `error`.
  `GET /datasets/imports/{id}` reads it; a run settled and unread for 30
  seconds is dropped. A run fails at once when the dataset name exists.
  Episodes are written 32 at a time, each batch in its own transaction; a
  failed run keeps the batches it committed.

### Curation

A curation is one opaque, versioned JSON payload per graph: the frontend owns
its schema and `encoding` version, and the backend stores, lists, and loads it
by identity without reading it (`GET /curations`, `GET /curations/{id}`). Save
(`POST /curations/save`) carries the revision the document was loaded at, 0
for one never saved, and returns the stored copy one revision higher; a
mismatch fails with `conflict`. Delete (`POST /curations/delete`) takes
`{id, revision}` and removes the curation only at that revision: a later save
fails it with `conflict`, a curation already gone with `not_found`; it answers
`null`. The browser drops the views a curation publishes before deleting it,
since only the payload names them. A payload over 32 MiB fails with
`invalid_input`. This is the only JSON route with a raised request-body bound,
twice the payload limit so a document's JSON framing always fits.

Publishing is a fill run. `POST /curation-fills` takes `{dataset, columns,
views}`: for each node that asks a model, in path order, its answer
`column`, the `episodes` it is filled for (a read-only statement listing the
episodes that reach the node, or listed ids), the `node` label the run
reports, and the `source` of its answers (a model call through OpenRouter,
the Modal services' progress scores or object tracks of a camera, or one
read-only statement over the dataset);
then the views, each `{name, statement}`: one plain identifier and the
read-only `SELECT` or `WITH` the browser compiled from an output. The backend
refuses an invalid name, statement, or dataset with `invalid_input`, and a
column the Modal service answers while its base URL or token is not saved with
`invalid_input`. It answers the run's `FillStatus` at once and works in the
background: each column is written for the episodes it does not hold yet, then
each view's statement is checked to plan and stored under its name, replacing
the one before. A run publishing a view replaces any earlier run publishing
it. `GET /curated-views` lists the published views, each `{name, statement}`. `GET /curation-fills` lists the
runs that publish views, `GET /curation-fills/{id}` reads one, and
`POST /curation-fills/{id}/stop` starts nothing more; what is in flight is
written. A Score progress preview starts the same run over the episodes it
previews, with no views. `POST /curations/unpublish` takes `{name}`, forgets
the view, and answers `null`; its stored answers stay.
`POST /curation-answers/filled` takes `{dataset, column}` and answers the
episodes the column holds an answer for, failed ones included, none before a
run first fills it. `POST /curation-answers/columns` takes a
`DatasetSelection` and answers the answer columns the dataset holds a table
for. See [curation](curation.md).

## Files and media

- `POST /project/import`: raw `.spiral-project` bytes with the project header. A
  body past 256 MiB fails with 413. The browser sends its native File without
  making a project-sized WebAssembly copy. Import returns `ProjectImportSummary`.
- `GET /project/export?endpoint=...&project_id=...`: version 6 project bundle
  download, a ZIP of `studio.sqlite3` (project-scoped control records, no
  backend credentials) and the project's MCAP files: the bytes of each one
  held inline, the pinned Hugging Face URI of each one imported from there.
  Import accepts only this version and also bounds total uncompressed contents
  to 256 MiB. See [project transfer](studio-sqlite.md#project-transfer).

Existing media/download error responses can contain plain text; JSON commands,
reads, and uploads consistently use the structured failure above.
There is no browser event stream or `/api/events` endpoint.

## Maintaining the boundary

Each backend feature owns its HTTP adapter, `http.rs`, beside the code it calls:
a manager for features with runtime state (training, curation), or the
request's `ProjectContext`. Handlers extract the request, call that owner, and
return its result; `OperationError` is the failure response. `Project` opens the
project a request names, before its body is read.

Each adapter returns two plain Axum routers, `reads()` and `commands()`.
`http/routes.rs` merges them and applies the one lifetime rule: commands run to
completion in their own task, reads stay in the request. `Created<T>` declares a
201 JSON response. A route that needs its own request bound sets it with Axum's
`DefaultBodyLimit` layer, as `/curations/save`, `/project/import`, and recording
uploads do.

Shared message types live in the corresponding features of `crates/common`.
Named responses stay named through application operations and UI presentation.
Router tests send ordinary HTTP requests through the real router.

The frontend's feature clients live beside their consuming features, for example
`activities/training/client.rs` and `robots/client.rs`. Each owns request
construction and the query domains its mutations invalidate. `ProjectClient`
owns session identity, request/response guards, and mounted task lifetime;
`RobotConnector` owns the robot connector's address and reachability;
`HttpRequest` owns serialization, failure decoding, and fetch cancellation. None
of them knows the set of feature operations.

Dioxus renders the UI and serves the application shell; it does not define or
implement the wire protocol.
