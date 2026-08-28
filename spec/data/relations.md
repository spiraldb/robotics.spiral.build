# Project data relations

## Outcome

Relations gives the user a quick, project-level view of the data they can work
with: which tables exist, how many rows each contains, which identity fields
they share, and what a few real rows look like. It describes the user's data,
not Studio's settings, run snapshots, derived caches, or managed-media
plumbing.

## User flow

1. Studio loads the active project's qualified relation names and Arrow schemas
   once at the shell boundary during startup, then shares that scoped resource
   with Data and Workflow.
2. The user opens Data and selects the Relations tab beside episode evidence.
3. The Relations view filters the shared catalog to user-facing project data
   and loads every exact table count with one combined read-only SQL query.
4. Studio shows likely joins between tables that share fields ending in
   `_id`, explicitly distinguishing those links from declared database
   constraints.
5. The user selects a table to inspect its schema and a bounded five-row
   preview.
6. The user may load that table's preview SQL into the existing Query panel for
   deeper inspection.

## Requirements

- **RELATIONS-001** Relations must be a movable Data workspace panel whose
  default home is a tab beside the primary episode evidence.
- **RELATIONS-002** The catalog must include native episode data and arbitrary
  imported project relations while excluding Studio-owned configuration,
  execution snapshots, Workflow caches, feature registry, and managed-media
  support relations.
- **RELATIONS-003** The catalog resource stored in `Studio` state must include
  each relation's qualified name, table type, Arrow columns, semantic extension
  names, and nullability. It must load once for the active project and be reused
  by Data, Query, and Workflow rather than copied into another context or
  activity controller.
- **RELATIONS-004** Relations must load all exact row counts through one
  Data-view-scoped read-only SQL statement containing one `COUNT(*)` branch per
  counted visible relation. The statement must remain within the shared query
  byte and row bounds and disclose relations beyond that bound. A failure of
  the combined metrics query must not hide the shared schemas or project
  tables; counts remain explicitly unavailable and independently retryable.
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

- Shared schema contract:
  [`spiral-robotics-api/src/relations.rs`](../../crates/spiral-robotics-api/src/relations.rs)
- Native catalog and schema discovery:
  [`storage/manager.rs`](../../crates/spiral-robotics-storage/src/manager.rs)
- Scoped shared catalog HTTP route:
  [`backend/http/routes/data.rs`](../../crates/spiral-robotics-backend/src/http/routes/data.rs)
- Studio-owned catalog resource:
  [`frontend/shell/state.rs`](../../crates/spiral-robotics-frontend/src/shell/state.rs)
- Data-scoped filtering, combined count SQL, and relationship projection:
  [`frontend/activities/data/relations.rs`](../../crates/spiral-robotics-frontend/src/activities/data/relations.rs)
- Data workspace registration:
  [`frontend/activities/data/screen.rs`](../../crates/spiral-robotics-frontend/src/activities/data/screen.rs)
