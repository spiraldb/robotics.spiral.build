# SQL query

## Outcome

Query lets the user inspect Spiral Robotics data with real relational semantics
inside the Data route.

## User flow

1. The user opens the Query panel.
2. The user chooses a preset or writes read-only SQL.
3. Studio runs a bounded query through the backend.
4. Studio shows a table without changing the Data layout.
5. A result that identifies an episode can open that episode.

## Requirements

- **QUERY-001** Data must provide a real SQL surface because relational
  inspection is a core feature.
- **QUERY-002** User SQL must be read-only.
- **QUERY-003** Query results must have a configured row and response-size
  limit.
- **QUERY-004** Query execution must preserve the Data route layout.
- **QUERY-005** Query execution must not hide Scene, Timeline, or camera panels
  unless the user changes the layout.
- **QUERY-006** A result that identifies an episode must allow the user to open
  that episode.
- **QUERY-007** Built-in query presets must use centralized relation and column
  names.
- **QUERY-008** Query errors must identify the failing relation or field without
  exposing credentials.

## Implementation ownership

- Query policy:
  [`spiral-robotics-core/src/query.rs`](../../crates/spiral-robotics-core/src/query.rs)
- Data query use case:
  [`backend/application/data.rs`](../../crates/spiral-robotics-studio-backend/src/backend/application/data.rs)
- SQL validation and ADBC adapter:
  [`backend/storage`](../../crates/spiral-robotics-studio-backend/src/backend/storage/)
- Query UI:
  [`frontend/screens/data`](../../crates/spiral-robotics-studio/src/frontend/screens/data/)

