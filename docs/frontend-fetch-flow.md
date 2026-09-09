# Frontend state comes from reads

The browser uses the backend-owned [HTTP API](http-api.md): fetched snapshots
and explicit JSON commands. Feature clients invalidate affected query domains
after mutations. A command response acknowledges the operation; subsequent reads
supply authoritative state. The browser owns drafts, selections, panel layout,
and cached results.

## Project and activity lifetime

[`shell/routes.rs`](../crates/frontend/src/shell/routes.rs) keys the project
subtree by its server session ID. Switching projects disposes that subtree,
including its resources, tasks, drafts, and selections. A failed project read
preserves the mounted identity and local drafts but withdraws readiness.

Each activity mounts on its first visit and remains mounted for that project.
Navigation hides inactive workspaces and passes an explicit `is_active` signal
to their controllers. Activity components own their state; there is no generic
retained-state registry. Hidden workspaces preserve drafts and results while
suspending reads and playback that require visibility.

The project shell owns connection/readiness, saved setup and settings,
notifications, and cross-activity navigation. Live owns capture state; Data owns
its resource selection and queries; Workflow owns its graph and Assistant drafts;
Training owns configuration and selected history; Rollout owns checkpoint and
robot selection; Settings owns its unsaved configuration and calibration state.

## Requests and refresh

[`client/project.rs`](../crates/frontend/src/client/project.rs) captures the
request's immutable session and rejects responses that no longer belong to it.
Backend admission also prevents operations from crossing project replacement.
Catalog-dependent reads share the backend's `catalog_available` readiness gate.

[`client/queries.rs`](../crates/frontend/src/client/queries.rs) owns query-domain
invalidation, cancellation, deadlines, and completion-based polling using Dioxus
resources. On-demand reads retain their result until input or invalidation
changes. Polling waits for completion before its next delay, so a timer never
cancels a slow request or overlaps another read. Changed inputs and invalidation
cancel obsolete reads. Mutations are never automatically retried.

Feature controllers choose their own intervals and visibility gates. Training,
Rollout, and Workflow can refresh faster during active work. Inactive activities
stay idle; reactivation reads current inputs. Browser backgrounding slows polling.
Data, Training, and Workflow do not poll hardware telemetry.

## Drafts and background work

Workflow refreshes preserve dirty drafts. Apply records the submitted draft and
accepted revision; a later fetch confirms the save. Edits made during that save
remain local. Settings similarly distinguishes fetched values from its draft.

Training history loads bounded details for the selected run. Recording charts
use cursor-based pages and a bounded retained sample window. Admitted backend
jobs and recovery own their lifetime independently of the browser activity.

There is no frontend event inbox or event reducer. Camera/media transfers and
the backend's provider SSE stream retain their own transport behavior.
