# Episode inspection: exact recording previews

Episode-scoped previews bind the selected native member ID as a single-row
Arrow UUID parameter. They read at most 100 messages ordered by log time and
sequence, expose the SQL and bound ID, and report truncation. The episode topic
list omits topics without messages in the selected recording. Cross-episode reads
remain explicit SQL queries. A direct read of an absent topic returns an empty preview.

The native Spiral driver must support comparison between returned UUIDv7
metadata and the generic UUID restored by Arrow parameter binding. Spiral PR
#7033 (`ed42714c9`) added that comparison while preserving all 16 UUID bytes.
Use the current [local driver](database-boundary.md); no filename, timestamp,
text-cast, or raw-storage filter substitutes for member identity.

The implementation and its request/parameter tests live in
[`datasets.rs`](../crates/backend/storage/src/datasets.rs). Video uses the same
exact member selection; see [camera playback](episode-playback.md).
