# Spiral data schema version

[`version.json`](version.json) is the machine-readable source for the
relational schema Studio writes to Spiral: each dataset's tables under
`datasets.<id>` ([datasets](../../../../docs/datasets.md#catalog-layout)).
Rust producers and consumers generate their schema-version constant from
this file. SQLite control-plane
migrations ([`../src/studio/migrations`](../src/studio/migrations)) and the
project bundle format have their own versions.

Change this version only when a persisted relation's shape or meaning changes.
Before changing it:

1. Implement the storage migration and compatibility behavior.
2. Update `current` once in `version.json`.
3. Add the matching `## Spiral schema N` entry to
   [`CHANGELOG.md`](CHANGELOG.md).
4. Update each external consumer's supported range according to real
   compatibility; do not widen ranges automatically.
5. Add producer, consumer, rejection, and artifact round-trip tests.

An internal refactor, bug fix, or deployment-only change does not require a
schema bump.
