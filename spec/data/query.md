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
- **QUERY-009** Query columns must preserve their Arrow extension name so the
  viewer can distinguish semantic media values from structurally similar
  structs, lists, and tensors.
- **QUERY-010** A decoded `spql.image.Image`, or an HWC unsigned-byte Arrow
  tensor shaped as height, width, and three or four channels, must render as a
  bounded inline image preview. The backend must encode that preview before
  JSON serialization rather than expanding pixels into numeric JSON values.
- **QUERY-011** A result that identifies one video asset by episode, stream,
  and segment must provide an inline browser video preview through the bounded
  byte-range media endpoint. Query JSON must not carry an MP4 payload.
- **QUERY-012** Results must remain one ordinary scrollable table. Semantic
  image and video values render directly in their corresponding cells rather
  than in a duplicate selected-row evidence surface. Headers show the field
  name and readable type; numbers align consistently, nulls remain explicit but
  quiet, and row count, elapsed time, limit, truncation, and SQL occupy one
  compact metadata strip.

## Implementation ownership

- Query policy:
  [`spiral-robotics-core/src/query.rs`](../../crates/spiral-robotics-core/src/query.rs)
- Data query use case:
  [`spiral-robotics-studio-application/src/data.rs`](../../crates/spiral-robotics-studio-application/src/data.rs)
- SQL validation and ADBC adapter:
  [`backend/storage`](../../crates/spiral-robotics-studio-backend/src/backend/storage/)
- Query UI:
  [`frontend/screens/data`](../../crates/spiral-robotics-studio/src/frontend/screens/data/)
