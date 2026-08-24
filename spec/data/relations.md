# Project data relations

## Outcome

Relations gives the user a quick, project-level view of the data they can work
with: which tables exist, how many rows each contains, which identity fields
they share, and what a few real rows look like. It describes the user's data,
not Studio's settings, run snapshots, derived caches, or managed-media
plumbing.

## User flow

1. The user opens Data and selects the Relations tab beside episode evidence.
2. Studio reads the active project's user-facing table schemas and exact row
   counts.
3. Studio shows likely joins between tables that share fields ending in
   `_id`, explicitly distinguishing those links from declared database
   constraints.
4. The user selects a table to inspect its schema and a bounded five-row
   preview.
5. The user may load that table's preview SQL into the existing Query panel for
   deeper inspection.

## Requirements

- **RELATIONS-001** Relations must be a movable Data workspace panel whose
  default home is a tab beside the primary episode evidence.
- **RELATIONS-002** The catalog must include native episode data and arbitrary
  imported project relations while excluding Studio-owned configuration,
  execution snapshots, Workflow caches, feature registry, and managed-media
  support relations.
- **RELATIONS-003** Each returned table must include its qualified name, table
  type, Arrow columns, semantic extension names, nullability, and an exact row
  count when a read-only `COUNT(*)` succeeds.
- **RELATIONS-004** A row-count failure for one table must not hide the other
  project tables. Studio must mark the catalog incomplete and state how many
  counts were unavailable.
- **RELATIONS-005** Relationship links must be derived only from exact shared
  identity-field names ending in `_id`. The UI must call them shared keys or
  likely joins and must not imply that Spiral declared a foreign-key
  constraint.
- **RELATIONS-006** Selecting a table must show its complete discovered schema
  and at most five preview rows through the same bounded, read-only query path
  and semantic media renderer used by Query.
- **RELATIONS-007** Loading a table in Query must change only the Query draft;
  it must not run SQL, move panels, or discard an existing result.
- **RELATIONS-008** Loading, empty, unavailable, partial, and populated states
  must remain contained within the Relations panel at the 1024 CSS-pixel
  desktop minimum.

## Implementation ownership

- Shared response contract:
  [`spiral-robotics-contracts/src/api.rs`](../../crates/spiral-robotics-contracts/src/api.rs)
- Catalog filtering, schemas, and counts:
  [`backend/storage/manager.rs`](../../crates/spiral-robotics-studio-backend/src/backend/storage/manager.rs)
- Data HTTP route:
  [`backend/http/routes/data.rs`](../../crates/spiral-robotics-studio-backend/src/backend/http/routes/data.rs)
- Relations panel and relationship projection:
  [`frontend/screens/data/relations.rs`](../../crates/spiral-robotics-studio/src/frontend/screens/data/relations.rs)
- Data workspace registration:
  [`frontend/screens/data/screen.rs`](../../crates/spiral-robotics-studio/src/frontend/screens/data/screen.rs)
