# Frontend state comes from reads

The browser uses the backend-owned [HTTP API](http-api.md): fetched snapshots
and explicit JSON commands. Feature clients invalidate affected query domains
after mutations. Resource reads supply authoritative backend state; document
Save responses acknowledge the submitted version directly. The browser owns drafts, selections, panel layout,
and cached results.

## Project and activity lifetime

The browser chooses the project it works on; the backend keeps no current
project. [`shell/project.rs`](../crates/frontend/src/shell/project.rs) takes the
first project from the saved connection and changes it only when this browser
saves another one, or when the saved endpoint moves.
[`shell/routes.rs`](../crates/frontend/src/shell/routes.rs) keys the project
subtree by that project. Switching projects disposes that subtree, including its
resources, tasks, drafts, and selections. An unreachable backend keeps the mount
and its drafts; views pause until the saved connection reads again.

Each activity mounts on its first visit and remains mounted for that project.
Navigation hides inactive workspaces and passes an explicit `is_active` signal
to their controllers. Activity components own their state; there is no generic
retained-state registry. Hidden workspaces preserve drafts and results while
suspending reads and playback that require visibility.

The project shell owns the selected project, backend liveness, saved setup and settings,
notifications, and cross-activity navigation. Live owns capture state; Data owns
its resource selection and queries; Curation owns its graph draft and preview;
Training owns configuration and selected history; Rollout owns checkpoint and
robot selection; Settings owns its unsaved configuration and calibration state.

## Requests and refresh

[`client/project.rs`](../crates/frontend/src/client/project.rs) names the mounted
project on every request and rejects responses that arrive after another project
was selected. Catalog-dependent reads wait until Studio is reachable and a Spiral
project is chosen.

[`client/queries.rs`](../crates/frontend/src/client/queries.rs) owns query-domain
invalidation, cancellation, deadlines, and completion-based polling using Dioxus
resources. On-demand reads retain their result until input or invalidation
changes. Polling waits for completion before its next delay, so a timer never
cancels a slow request or overlaps another read. Changed inputs and invalidation
cancel obsolete reads. Mutations are never automatically retried.

Feature controllers choose their own intervals and visibility gates. Training
and Rollout can refresh faster during active work. Inactive activities
stay idle; reactivation reads current inputs. Browser backgrounding slows polling.
Data, Curation, and Training do not poll hardware telemetry.

## Drafts and background work

Workflow owns its entire draft and Assistant loop locally. It fetches documents
by ID and submits a complete graph once when starting a run. Save acknowledges
the submitted document's saved version while preserving subsequent local edits.
Only execution records are polled; graph drafts and conversations are never
reconciled from backend editor snapshots. Settings similarly distinguishes fetched values from its draft.

Robot setup joins saved robot documents and native hardware observations locally.
Port identification compares two ordinary scans in the browser. Port/camera edits
preserve unknown configuration fields locally and submit the complete document
with its baseline. `RobotDraft` owns creation, import, edits, and validation;
`RobotClient` only transports resources and invalidates queries. The backend
performs transactional comparison and persistence.
Recording requests include the frontend-selected leader/follower identities.

Training reads the run catalog and fetches bounded details by ID; selected tabs
never appear in backend requests. Rollout observes and stops executions by ID.
Configuration drafts remain independent of submitted jobs. Recording charts
use cursor-based pages and a bounded retained sample window. Backend jobs and
recovery own their lifetime independently of the browser activity.
`ProjectContext::execute` runs robot, recording, training, rollout, and
workflow-save commands to completion, retaining their project even when the
HTTP caller disconnects.

There is no frontend event inbox or event reducer. Camera/media transfers and
the request-scoped model completion stream retain their own transport behavior.
