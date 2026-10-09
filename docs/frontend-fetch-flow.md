# Frontend state comes from reads

The browser uses the backend-owned [HTTP API](http-api.md): fetched snapshots
and explicit JSON commands. Feature clients invalidate affected query domains
after mutations. Resource reads supply authoritative backend state; document
Save responses acknowledge the submitted version directly. The browser owns drafts, selections, panel layout,
and cached results.

## Project and activity lifetime

The browser chooses the workspace it works in, named by its Spiral project; the
backend keeps no current project.
[`shell/project.rs`](../crates/frontend/src/shell/project.rs) takes the first
workspace from the saved connection and changes it only when this browser
switches to another, when the saved endpoint moves, or when a request finds its
workspace deleted. [`shell/routes.rs`](../crates/frontend/src/shell/routes.rs)
keys the project subtree by that project. Switching workspaces disposes that
subtree, including its resources, tasks, drafts, and selections. An unreachable
backend keeps the mount and its drafts; views pause until the saved connection
reads again.

Each activity mounts on its first visit and remains mounted for that project.
Navigation hides inactive workspaces and passes an explicit `is_active` signal
to their controllers. Activity components own their state; there is no generic
retained-state registry. Hidden workspaces preserve drafts and results while
suspending reads and playback that require visibility.

The project shell owns the selected workspace and its settings
([`shell/workspace.rs`](../crates/frontend/src/shell/workspace.rs)), backend
liveness, the robot connector, notifications, and cross-activity navigation. Data owns
its resource selection and queries; Curation owns its graph draft and preview;
Training owns configuration and selected history; Rollout owns capture state,
the next episode, checkpoint and robot selection, and calibration state; Settings
owns its unsaved configuration.

## Requests and refresh

[`client/project.rs`](../crates/frontend/src/client/project.rs) names the mounted
project on every request and rejects responses that arrive after another project
was selected. Catalog-dependent reads wait until Studio is reachable and a
workspace is chosen.
[`client/robot_connector.rs`](../crates/frontend/src/client/robot_connector.rs)
sends robot and recording requests to the robot connector at the
address the workspace keeps, and asks whether it answers only while a hardware
view is active, so a browser that never opens one never reaches for a local
network address.

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

Curation owns its graph draft locally. It fetches documents by ID and submits
the complete graph on an explicit Save, which acknowledges the saved revision
while preserving subsequent local edits. Only preview runs are polled; the
draft is never reconciled from a backend snapshot. Settings similarly
distinguishes fetched values from its draft.

Robot setup joins saved robot documents and native hardware observations locally.
Port identification compares two ordinary scans in the browser. Port/camera edits
preserve unknown configuration fields locally and submit the complete document
with its baseline. `RobotDraft` owns creation, import, edits, and validation;
`RobotClient` only transports resources and invalidates queries. The robot
connector performs the comparison and persistence.
Recording requests name their driver: the frontend-selected leader/follower
identities for teleoperation. After an episode stops, the robot connector uploads
its MCAP file to Studio; Rollout polls Studio's waiting episodes while it is
active, stores each as it arrives, and lists one whose store failed, to try
again.

Training reads the run catalog and fetches bounded details by ID; selected tabs
never appear in backend requests.
Configuration drafts remain independent of submitted jobs. Recording charts
use cursor-based pages and a bounded retained sample window. Backend jobs and
recovery own their lifetime independently of the browser activity. Every
command runs to completion in its own backend task, retaining its project even
when the HTTP caller disconnects.

There is no frontend event inbox or event reducer. Camera transfers keep their
own transport behavior.
