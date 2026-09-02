# Storage schema version

[`version.json`](version.json) is the machine-readable source for the current
relational schema written by Studio. Rust producers and consumers generate
their schema-version constant from this file; external workers package it
alongside their own protocol metadata.

Change this version only when a persisted relation's shape or meaning changes.
Before changing it:

1. Implement the storage migration and compatibility behavior.
2. Update `current` once in `version.json`.
3. Add the matching `## Spiral schema N` entry to
   [`CHANGELOG.md`](CHANGELOG.md).
4. Update each external consumer's supported range according to real
   compatibility; do not widen ranges automatically.
5. Add producer, consumer, rejection, and artifact round-trip tests.
6. Update `./spec` after the behavior exists.

An internal refactor, bug fix, or deployment-only change does not require a
schema bump.
