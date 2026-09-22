# Dataset and recording boundary

A dataset is a Spiral resource backed by a set of MCAP files. Studio recording and
external-file import are producers of those files, not different dataset kinds.

```text
Live capture → complete MCAP bytes ─┐
                                   ├→ MCAP sources → Spiral dataset → consumers
External files → pinned Blob URIs ─┘
```

## Ownership

- **Recording** owns hardware sampling, capture clocks, pause/resume, encoder
  segments, and file finalization. It writes an immutable local MCAP before
  publication so a failed upload does not lose the take.
- **MCAP source publication** owns the managed source relation and dataset
  definition. Local bytes and external Blob references use the same MCAP producer,
  definition, refresh, and membership checks. File keys are opaque and scoped to
  their destination. Neither a filename pattern nor Studio metadata is required.
- **Dataset consumers** use dataset identity/version, member UUID, topic/schema
  revision, timestamps, and values. They discover capabilities from the actual
  schema. They do not read the ingestion table, reconstruct MCAP filenames, or
  require `/studio/episode`, `/studio/streams/*`, or Studio's capture DTOs.

The `dataset_source` table and `dataset_source_<name>` views are Studio's managed
input implementation, used by both capture and external import. Its historical
`recording` column is an opaque source key, not an episode identity. Append/remove
and project transfer validate ownership of this input definition before mutation.
The general catalog and readers accept dataset paths independently of that naming
convention. An arbitrary external dataset is not automatically writable by Studio.

## Live review versus published data

Live's current-take preview is a recording-session operation.
`RecordingBuffer` retains only the latest completed/checkpointed take and has no
project connection or persisted-file fallback. Camera preview retains the same
session's encoded packets after publication and releases them when a new capture
starts. This preserves immediate pause/stop review without interpreting stored
MCAP files in Studio.

`RecordingSessionManager` protects one lifecycle enum with a standard mutex.
Opening, recording, capture changes, finalization, and review each own their valid
state. Pause/resume and finalization claim the capture before changing hardware
state, then perform encoder waits outside the mutex. An unfinished operation
returns its capture to the session on drop, including pending encoder tasks,
completed packets, and errors. Finishing publication transitions to review and
releases the catalog lease together, even when publication reports an error.

These endpoints live under `/recording/takes/{id}`. Their IDs identify session
captures; they are not dataset member UUIDs. After a session is replaced or Studio
restarts, persisted content is inspected through Data's dataset/member/topic APIs.

The producer may put Studio metadata into its MCAP files. That metadata is ordinary
optional content once published. Studio’s old MCAP reader and its reader-specific
tests are removed. Dataset reads and schema decoding go through Spiral.

## Audit and verification

| Area | Boundary after cleanup |
| --- | --- |
| Data browser, topic inspection, cameras, numeric timelines | Dataset/member/topic APIs; no capture-header or source-filename lookup |
| Live take inspection, seeking, camera preview | Recording-owned APIs and an in-memory current-session buffer |
| Workflow | Reads Spiral relations; source provenance is not a workflow contract |
| Training | Worker owns dataset reads; unavailable MCAP image decoding remains explicit, with no Studio decoder fallback |
| Local capture and Hugging Face import | Shared dataset definition, Blob representation, refresh, and membership validation |
| Project transfer | Preserves managed source files and dataset definitions, without decoding capture headers |

Tests cover session-buffer isolation, completed camera preview and replacement,
MCAP writer output, and the common publication path with both Studio camera/
joint files and a sensor-only MCAP from another producer. The integration test uses
an arbitrary nested source key without an extension, verifies idempotent retries,
and verifies the same key in another dataset survives removal from the first.

This cleanup does not change the stored source-table layout or convert existing
files. Workflow-model unification and the tool-trait refactor are separate follow-up
work.
