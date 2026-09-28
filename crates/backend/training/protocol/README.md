# Training runtime protocol version

[`version.json`](version.json) is the machine-readable source for the
Studio/training-runtime protocol version and the storage schema range that the
runtime can consume. Rust generates its protocol constants from this file, and
the Python runtime packages the same file.

Compatibility rules:

- Bump `minor` for an additive endpoint or optional field that preserves all
  behavior in earlier revisions. Studio accepts a runtime with the same major
  and an equal or newer minor.
- Bump `major` when removing or renaming an endpoint or field, changing a
  field's type or meaning, or adding a required field. Reset `minor` to zero.
- Update `storage_schema` only to describe compatibility that the runtime
  actually implements. The current storage schema must remain inside this
  range.
- A storage schema bump does not by itself require a training API bump. The
  selected schema version travels in the job request and checkpoint manifest.
- Internal refactors, bug fixes, and deployment-only changes do not require a
  protocol bump.

Before changing the protocol, implement parsing and compatibility behavior,
update `version.json`, add the matching `## Training API M.N` entry to
[`CHANGELOG.md`](CHANGELOG.md), and add cross-process tests.
