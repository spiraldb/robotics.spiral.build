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
   `/keys`, `/proxy`, `/account` reads, `/workspaces`, `/settings/connection`, and
   `POST /datasets/hugging-face/inspect` need neither header.
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

Every request carries a browser's session cookie, `studio_session`; sign in at
`http://localhost:3000` and copy it from the browser's developer tools. Read
the session's connection, then its project's training runs:

```sh
curl -b "studio_session=$STUDIO_SESSION" http://localhost:3000/api/settings/connection
curl -b "studio_session=$STUDIO_SESSION" \
  -H 'X-Spiral-Endpoint: https://api.spiraldb.dev' -H 'X-Spiral-Project: <project_id>' \
  http://localhost:3000/api/training/runs
```

Submit a read-only SQL query with an explicit row bound:

```sh
curl -X POST http://localhost:3000/api/queries -b "studio_session=$STUDIO_SESSION" \
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

Every browser signs in to Spiral itself, on Spiral's WorkOS sign-in page, with
an authorization code and PKCE. The backend keeps each session's tokens and
names the session in an HttpOnly `studio_session` cookie; responses never
include a token. Every `/api` request needs a live session and otherwise fails
with `401` and the code `unauthenticated`. Opening a project asks Spiral
whether the session's user may view it.

The sign-in pages are outside `/api`, because the browser navigates to them:

- `GET /sign-in?return_to=<path>` sends the browser to Spiral's sign-in page.
- `GET /callback` receives it back, starts the session, and returns the browser
  to `return_to`, a path on this Studio.

The account endpoints:

- `GET /account` returns the `SpiralIdentity`: `email` and `organization`.
- `GET /account/organizations` and `GET /account/projects` list the user's
  organizations and the projects they can open in the current one.
- `POST /account/sign-out` and `PUT /account/organization`
  (`{"organization_id": …}`) name the mounted project. They refuse while any of
  the session's projects is training, then close the
  session's open projects. Signing out ends the session and its cookie;
  another organization forgets the session's workspace.

### Workspaces

A workspace is one Spiral project under the name Studio shows for it, with
everything Studio keeps for that project: its curations, preferences, robot
connector, and training runtime. Every signed-in user sees the workspaces whose
projects Spiral lets them open; Spiral alone decides who may open, change, or
delete which. Each backend keeps its own workspaces, in its
[Studio database](studio-sqlite.md).

- `GET /workspaces` lists the `Workspace`s (`project_id`, `name`) the user can
  open, by name.
- `POST /workspaces` (`NewWorkspace`: `{"name": …, "project_id": … | null}`)
  makes a workspace on an existing project the user can open, or with a null
  `project_id` on a new project Spiral creates in the user's organization. A
  project has at most one workspace: another fails with `conflict`.
- `DELETE /workspaces/{project_id}` forgets the workspace and every Studio
  record of its project, and closes it in every browser. The Spiral project and
  its data stay. It refuses while the project is training.
- `GET /settings/connection` returns the Spiral endpoints and the project of the
  workspace the session works in: the one it chose while that exists, else the
  oldest workspace the user can open, else empty.
- `PUT /settings/connection` (`{"project_id": …}`) makes the session work in
  that workspace and returns the same shape.
- `GET /settings` returns the named workspace's `DataSettings`: the Spiral
  endpoints and project, its `name`, `training` runtime, and `robot_connector`
  address.
- `PUT /settings` (`{"name": …, "training": {"endpoint": …}}`) renames the named
  workspace and sets its training runtime; `PUT /settings/robot-connector`
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

### Training and rollout

Both are parked: starting a training run fails until Spiral decodes MCAP video
into training images, so Rollout has only existing checkpoints to run.
Training selection and assembly of catalog/detail responses are frontend state.
Rollouts run on the robot connector. `GET /training/runs/{id}/checkpoints/{step}/policy`
answers that checkpoint as a policy envelope
(`application/vnd.spiral.policy-artifact`), which the browser writes to the robot
connector unchanged before starting a rollout of it.

### Recordings

Takes are recorded on the robot connector. The browser carries each finished
one to Studio: `POST /datasets/{dataset}/recordings/{episode}` takes the take's
MCAP bytes (at most 256 MiB, one Spiral row) and stores them as
`{episode}.mcap` in the MCAP dataset `dataset`, creating it with its first
take. Storing the same take again only refreshes the dataset. Published
dataset content uses `/datasets/episode-topics` and episode queries with
explicit dataset/member identities; no recording-header fallback exists.

### Hugging Face import

Dataset import accepts public Hugging Face repositories containing MCAP files. The
request has `dataset_id`, `repository`, and optional `revision` (default `main`);
imports create `dataset_files_<dataset_id>`, a table of the repository's MCAP
files, and derive `dataset_<dataset_id>` from it in one transaction, copying
every topic into Spiral.
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
mismatch fails with `conflict`. Delete (`POST /curations/delete`) takes
`{id, revision}` and removes the curation only at that revision: a later save
fails it with `conflict`, a curation already gone with `not_found`; it answers
`null`. The browser drops the views a curation publishes before deleting it,
since only the payload names them. A payload over 32 MiB fails with
`invalid_input`. This is the only JSON route with a raised request-body bound,
twice the payload limit so a document's JSON framing always fits.

Publishing is a fill run. `POST /curation-fills` takes `{dataset, columns,
views}`: for each node that asks a model, in path order, its answers-table
`column`, the `episodes` it is filled for (a read-only statement listing the
episodes that reach the node, or listed ids), the `node` label the run
reports, and the `source` of one episode's answer (a read-only statement with
the episode bound as `$1`, or the Modal service's progress scores of a camera);
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
the view, and answers `null`; its stored answers stay. See
[curation](curation.md).

## Files and media

- `POST /project/import`: raw `.spiral-project` bytes with the project header. A
  body past 256 MiB fails with 413. The browser sends its native File without
  making a project-sized WebAssembly copy. Import returns `ProjectImportSummary`.
- `GET /project/export?endpoint=...&project_id=...`: version 4 project bundle
  download, a ZIP of `studio.sqlite3` (project-scoped control records, no
  backend credentials) and the project's MCAP sources and checkpoint artifacts.
  Import accepts only this version and also bounds total uncompressed contents
  to 256 MiB. See [project transfer](studio-sqlite.md#project-transfer).
- `POST /datasets/{dataset}/recordings/{episode}`: one take's MCAP bytes, carried
  from the robot connector; see [Recordings](#recordings).
- `GET /training/runs/{id}/checkpoints/{step}/policy`: one checkpoint as a policy
  envelope, for the robot connector.

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
