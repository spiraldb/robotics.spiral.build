//! How the checks keep what they measured: one column of the dataset's
//! answers table per check, and in it each episode's cell, listing the parts
//! of the recording the check measured in that episode — a stream, a camera,
//! an arm. A cell holds that episode's measurements alone, so appending
//! episodes checks only the new ones; whatever compares episodes with each
//! other — a typical rate, the furthest a joint reached — is judged from
//! every cell when the report is read.
//!
//! The browser composes each check's statement; a backend fill run reads it
//! over every episode at once and writes the column, as a curation's fill
//! writes the columns its nodes ask for.
use super::stream_reads::CheckedStreams;
use crate::activities::data::DatasetClient;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use spiral_robotics_common::curation::{
    AnswerColumn, AnswerSource, AnswerTable, ColumnFill, FillEpisodes,
};
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};
use spiral_robotics_common::sql::SqlLiteral;

/// The version of the checks a column was filled by. Changing what any
/// check measures or keeps bumps it, and every dataset is checked again.
const DEFINITION: u32 = 2;
/// The most rows one read of the answers table answers with.
const PAGE_ROWS: usize = 1000;

/// Every check, each one column of the answers table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CheckKind {
    Rates,
    Frames,
    Clock,
    Lag,
    Limits,
    Spikes,
    Pauses,
    Stuck,
    Grippers,
}

impl CheckKind {
    /// What the check's column is named after.
    fn measure(self) -> &'static str {
        match self {
            Self::Rates => "check_rates",
            Self::Frames => "check_frames",
            Self::Clock => "check_clock",
            Self::Lag => "check_lag",
            Self::Limits => "check_limits",
            Self::Spikes => "check_spikes",
            Self::Pauses => "check_pauses",
            Self::Stuck => "check_stuck",
            Self::Grippers => "check_grippers",
        }
    }
    /// What a run says while it fills the check's column.
    pub(super) fn doing(self) -> &'static str {
        match self {
            Self::Rates => "Measuring recording rates",
            Self::Frames => "Counting camera frames",
            Self::Clock => "Pairing camera and stream clocks",
            Self::Lag => "Measuring state–action lag",
            Self::Limits => "Checking joint limits",
            Self::Spikes => "Looking for spikes",
            Self::Pauses => "Finding pauses",
            Self::Stuck => "Looking for stuck joints",
            Self::Grippers => "Classifying gripper closes",
        }
    }
}

/// The statement filling one check's column. Each part — a stream, a
/// camera, an arm — is a statement answering `id` and the part's `fields`
/// at most once per episode; an episode's cell lists every part that
/// answered for it, each named in `part`.
pub(super) struct PartsStatement {
    fields: &'static [&'static str],
    parts: Vec<(String, String)>,
}

impl PartsStatement {
    pub(super) fn new(fields: &'static [&'static str]) -> Self {
        Self {
            fields,
            parts: Vec::new(),
        }
    }
    /// One more part, `name`, answered by `statement`.
    pub(super) fn part(mut self, name: impl Into<String>, statement: String) -> Self {
        self.parts.push((name.into(), statement));
        self
    }
    /// The statement over every episode, answering `episode_id` and its
    /// cell in `answer`; `None` when there is no part to measure.
    fn sql(&self) -> Option<String> {
        if self.parts.is_empty() {
            return None;
        }
        let selected = std::iter::once("id")
            .chain(self.fields.iter().copied())
            .collect::<Vec<_>>()
            .join(", ");
        let parts = self
            .parts
            .iter()
            .map(|(name, statement)| {
                format!(
                    "SELECT {} AS part, {selected} FROM ({statement}) x",
                    SqlLiteral::text(name)
                )
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        let packed = std::iter::once("part := part".to_owned())
            .chain(
                self.fields
                    .iter()
                    .map(|field| format!("{field} := {field}")),
            )
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!(
            "SELECT id AS episode_id, spql.list.collect(STRUCT_PACK({packed})) AS answer \
             FROM ({parts}) parts GROUP BY id"
        ))
    }
}

/// One check's column: the statement that fills it, and its name, which
/// hashes that statement, so a check whose inputs changed — another stream,
/// other joint roles — fills a column of its own.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct CheckColumn {
    kind: CheckKind,
    column: AnswerColumn,
    statement: String,
}

impl CheckColumn {
    /// `kind`'s column, filled by `parts`; `None` when there is nothing to
    /// measure.
    pub(super) fn of(kind: CheckKind, parts: PartsStatement) -> Result<Option<Self>, String> {
        let Some(statement) = parts.sql() else {
            return Ok(None);
        };
        let column = AnswerColumn::of(kind.measure(), &(DEFINITION, &statement))?;
        Ok(Some(Self {
            kind,
            column,
            statement,
        }))
    }
    pub(super) fn kind(&self) -> CheckKind {
        self.kind
    }
    /// The run's part that fills this column for every episode `streams`
    /// reads.
    pub(super) fn fill(&self, streams: CheckedStreams<'_>) -> ColumnFill {
        ColumnFill::new(
            self.kind.doing(),
            self.column.clone(),
            FillEpisodes::Reaching(format!(
                "SELECT id AS episode_id FROM {}",
                streams.episodes()
            )),
            AnswerSource::DatasetStatement {
                statement: self.statement.clone(),
            },
        )
    }
}

/// What the report reads of the answers table: each episode's id beside its
/// cell of every check column filled so far.
pub(super) struct CheckRows {
    filled: Vec<String>,
    rows: Vec<serde_json::Map<String, serde_json::Value>>,
}

impl CheckRows {
    /// The cells `columns` hold in `dataset`'s answers table, a page of
    /// episodes at a time. A column no run has filled yet is not read.
    pub(super) async fn read(
        client: &DatasetClient,
        dataset: &DatasetSelection,
        columns: &[CheckColumn],
    ) -> Result<Self, String> {
        let table = AnswerTable::of(dataset)?.quoted();
        let text = |error: crate::client::CommandError| error.to_string();
        let stored = client
            .run_spiral_query(format!("SELECT * FROM {table} LIMIT 0"), 1)
            .await
            .map_err(text)?;
        let filled = columns
            .iter()
            .map(|column| column.column.as_str().to_owned())
            .filter(|name| stored.columns().iter().any(|stored| stored.name() == name))
            .collect::<Vec<_>>();
        let selected = std::iter::once("episode_id".to_owned())
            .chain(filled.iter().cloned())
            .collect::<Vec<_>>()
            .join(", ");
        let mut rows = Vec::new();
        loop {
            let page = client
                .run_spiral_query(
                    format!(
                        "SELECT {selected} FROM {table} ORDER BY episode_id LIMIT {PAGE_ROWS} OFFSET {}",
                        rows.len()
                    ),
                    PAGE_ROWS,
                )
                .await
                .map_err(text)?
                .into_rows();
            let more = page.len() == PAGE_ROWS;
            rows.extend(page);
            if !more {
                break;
            }
        }
        Ok(Self { filled, rows })
    }
    /// Whether a run has filled `column` for some episode.
    pub(super) fn holds(&self, column: &CheckColumn) -> bool {
        self.filled
            .iter()
            .any(|filled| filled == column.column.as_str())
    }
    /// How many episodes `columns` hold no cell for yet, counting each
    /// episode once.
    pub(super) fn unchecked(&self, columns: &[CheckColumn]) -> usize {
        self.rows
            .iter()
            .filter(|row| {
                columns.iter().any(|column| {
                    row.get(column.column.as_str())
                        .is_none_or(|cell| cell.is_null())
                })
            })
            .count()
    }
    /// How many episodes the answers table holds.
    pub(super) fn episodes(&self) -> usize {
        self.rows.len()
    }
    /// The cells of `kind`'s column among `columns`, its parts read as `P`;
    /// none when the check has no column, having nothing to measure.
    pub(super) fn cells<P: DeserializeOwned>(
        &self,
        columns: &[CheckColumn],
        kind: CheckKind,
    ) -> Result<CheckCells<P>, String> {
        match columns.iter().find(|column| column.kind == kind) {
            Some(column) => CheckCells::read(self, column),
            None => Ok(CheckCells { cells: Vec::new() }),
        }
    }
}

/// Each episode's cell of one check's column, its parts read as `P`.
pub(super) struct CheckCells<P> {
    cells: Vec<(EpisodeId, Vec<P>)>,
}

/// A cell as the answers table stores it: a NULL value is an episode the
/// check found nothing to measure in.
#[derive(Deserialize)]
struct StoredCell<P> {
    value: Option<Vec<P>>,
}

#[derive(Deserialize)]
struct RowEpisode {
    episode_id: EpisodeId,
}

impl<P: DeserializeOwned> CheckCells<P> {
    /// `column`'s cells in `rows`; an episode without one lists no parts.
    fn read(rows: &CheckRows, column: &CheckColumn) -> Result<Self, String> {
        let reading = |error: serde_json::Error| {
            format!(
                "Reading the {} check's cells: {error}",
                column.kind.measure()
            )
        };
        let mut cells = Vec::with_capacity(rows.rows.len());
        for row in &rows.rows {
            let RowEpisode { episode_id } =
                serde_json::from_value(serde_json::Value::Object(row.clone())).map_err(reading)?;
            let parts = match row.get(column.column.as_str()) {
                None | Some(serde_json::Value::Null) => Vec::new(),
                Some(cell) => serde_json::from_value::<StoredCell<P>>(cell.clone())
                    .map_err(reading)?
                    .value
                    .unwrap_or_default(),
            };
            cells.push((episode_id, parts));
        }
        Ok(Self { cells })
    }
}

impl<P> CheckCells<P> {
    /// Every part each episode measured, beside its episode.
    pub(super) fn parts(&self) -> impl Iterator<Item = (&EpisodeId, &P)> {
        self.cells
            .iter()
            .flat_map(|(episode, parts)| parts.iter().map(move |part| (episode, part)))
    }
    /// Cells built in a test, as a column would hold them.
    #[cfg(test)]
    pub(super) fn fixture(cells: Vec<(EpisodeId, Vec<P>)>) -> Self {
        Self { cells }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_check_lists_each_episodes_parts_in_one_statement_named_for_it() {
        let parts = PartsStatement::new(&["messages", "first_ns"])
            .part(
                "/a",
                "SELECT id, count(*) AS messages, min(log_time) AS first_ns FROM ta GROUP BY id"
                    .into(),
            )
            .part(
                "/b's",
                "SELECT id, count(*) AS messages, min(log_time) AS first_ns FROM tb GROUP BY id"
                    .into(),
            );
        let column = CheckColumn::of(CheckKind::Rates, parts).unwrap().unwrap();
        assert_eq!(
            column.statement,
            "SELECT id AS episode_id, spql.list.collect(STRUCT_PACK(part := part, messages := messages, first_ns := first_ns)) AS answer \
             FROM (SELECT '/a' AS part, id, messages, first_ns FROM (SELECT id, count(*) AS messages, min(log_time) AS first_ns FROM ta GROUP BY id) x \
             UNION ALL SELECT '/b''s' AS part, id, messages, first_ns FROM (SELECT id, count(*) AS messages, min(log_time) AS first_ns FROM tb GROUP BY id) x) parts GROUP BY id"
        );
        assert!(column.column.as_str().starts_with("check_rates_"));
        // Another statement is another column; nothing to measure is none.
        let other = CheckColumn::of(
            CheckKind::Rates,
            PartsStatement::new(&["messages"])
                .part("/a", "SELECT id, 1 AS messages FROM ta".into()),
        )
        .unwrap()
        .unwrap();
        assert_ne!(other.column, column.column);
        assert!(
            CheckColumn::of(CheckKind::Rates, PartsStatement::new(&["messages"]))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn cells_read_each_episodes_parts_and_an_empty_cell_lists_none() {
        #[derive(Debug, Deserialize, PartialEq)]
        struct Part {
            part: String,
            messages: u64,
        }
        let column = CheckColumn::of(
            CheckKind::Rates,
            PartsStatement::new(&["messages"])
                .part("/a", "SELECT id, 1 AS messages FROM ta".into()),
        )
        .unwrap()
        .unwrap();
        let name = column.column.as_str().to_owned();
        let row = |id: u32, cell: serde_json::Value| {
            let mut row = serde_json::Map::new();
            row.insert("episode_id".into(), format!("{id:032x}").into());
            row.insert(name.clone(), cell);
            row
        };
        let rows = CheckRows {
            filled: vec![name.clone()],
            rows: vec![
                row(
                    1,
                    serde_json::json!({"value": [{"part": "/a", "messages": 3}]}),
                ),
                row(2, serde_json::json!({"value": null})),
                row(3, serde_json::Value::Null),
            ],
        };
        assert!(rows.holds(&column));
        assert_eq!(rows.unchecked(std::slice::from_ref(&column)), 1);
        let cells = CheckCells::<Part>::read(&rows, &column).unwrap();
        let parts = cells.parts().map(|(_, part)| part).collect::<Vec<_>>();
        assert_eq!(
            parts,
            [&Part {
                part: "/a".into(),
                messages: 3
            }]
        );
    }
}
