# Versioned boundaries

[`boundaries.json`](boundaries.json) is the single machine-readable source for
cross-process and persisted-schema versions. Rust build scripts generate public
constants from it, and the Python runtime loads the same file. Do not copy a
current version number into implementation code.

The versions answer different questions:

- `spiral_schema.current` identifies the relational shape and meaning written
  by Studio.
- `training_api.major.minor` identifies the Studio/runtime wire contract.
- `training_api.spiral_schema` is the schema range that this runtime
  implementation can actually consume.

## Compatibility rules

- Do not bump a boundary for an internal refactor, bug fix, or deployment-only
  change.
- Bump the training API **minor** for an additive endpoint or optional field
  that preserves all behavior in earlier revisions. A Studio client accepts a
  runtime with the same major and an equal or newer minor.
- Bump the training API **major** when removing or renaming an endpoint or
  field, changing a field type or meaning, or adding a required field. Reset
  minor to zero.
- Bump the Spiral schema only when a persisted relation's shape or semantics
  change. Update the runtime's supported range according to real compatibility;
  do not automatically widen it.
- A Spiral schema bump does not by itself require a training API bump. The
  selected schema version travels in the job request and checkpoint manifest.

## Bump checklist

1. Implement parsing and compatibility behavior before changing the descriptor.
2. Change the version once in `boundaries.json`.
3. Add the matching entry to [`CHANGELOG.md`](CHANGELOG.md); builds reject a
   descriptor with no changelog heading.
4. Add producer, consumer, rejection, and artifact round-trip tests.
5. Update `./spec` after the behavior exists.
6. Run Rust tests, Python tests, and build the Python wheel to verify the
   descriptor is packaged.

The descriptor intentionally fails closed: its builders and Python loader
reject unknown fields, invalid ranges, and a current Spiral schema outside the
runtime support range.
