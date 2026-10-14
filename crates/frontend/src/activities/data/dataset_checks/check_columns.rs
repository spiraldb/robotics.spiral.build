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
//! writes the columns its nodes ask for. The report judges the columns in
//! Spiral too — medians, thresholds, the episodes a finding names — so only
//! what it shows comes back to the browser, however many episodes there are.
use super::stream_reads::CheckedStreams;
use crate::activities::data::DatasetClient;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use spiral_robotics_common::curation::{
    AnswerColumn, AnswerSource, AnswerTable, ColumnFill, FillEpisodes,
};
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::sql::SqlLiteral;

/// The version of the checks a column was filled by. Changing what any
/// check measures or keeps bumps it, and every dataset is checked again.
const DEFINITION: u32 = 2;
/// The most rows one judgment of the answers table answers with.
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
    fields: &'static [&'static str],
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
            fields: parts.fields,
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
    /// This column's parts in the answers table `table`, as a relation:
    /// `episode_id`, `part` and the check's fields, one row per part of every
    /// episode. An episode with no cell, or one the check found nothing to
    /// measure in, has no row.
    fn parts_in(&self, table: &str) -> String {
        let fields = std::iter::once("part")
            .chain(self.fields.iter().copied())
            .map(|field| format!("p.{field} AS {field}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "(SELECT a.episode_id AS episode_id, {fields} FROM {table} a CROSS JOIN LATERAL UNNEST(a.{}.value) p)",
            self.column
        )
    }
}

/// The dataset's answers table as the report reads it: which check columns
/// runs have filled, how many episodes no run has checked, and each check's
/// parts as a relation its judgment reads in Spiral.
pub(super) struct CheckTable {
    client: DatasetClient,
    table: String,
    filled: Vec<String>,
    episodes: u64,
    unchecked: u64,
}

/// How many episodes the answers table holds, and how many lack a cell in
/// some filled check column.
#[derive(Deserialize)]
struct EpisodeCounts {
    episodes: Option<u64>,
    unchecked: Option<u64>,
}

impl CheckTable {
    /// Which of `columns` runs have filled in `dataset`'s answers table, and
    /// how many of its episodes they hold no cell for.
    pub(super) async fn read(
        client: &DatasetClient,
        dataset: &DatasetSelection,
        columns: &[CheckColumn],
    ) -> Result<Self, String> {
        let table = AnswerTable::of(dataset).table().to_string();
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
        let lacking = match filled.as_slice() {
            [] => "1".to_owned(),
            filled => format!(
                "CASE WHEN {} THEN 1 ELSE 0 END",
                filled
                    .iter()
                    .map(|column| format!("{column} IS NULL"))
                    .collect::<Vec<_>>()
                    .join(" OR ")
            ),
        };
        let mut table = Self {
            client: client.clone(),
            table,
            filled,
            episodes: 0,
            unchecked: 0,
        };
        let counts = table
            .rows::<EpisodeCounts>(format!(
                "SELECT count(*) AS episodes, sum({lacking}) AS unchecked FROM {}",
                table.table
            ))
            .await?;
        if let Some(counts) = counts.first() {
            table.episodes = counts.episodes.unwrap_or_default();
            table.unchecked = counts.unchecked.unwrap_or_default();
        }
        Ok(table)
    }
    /// Whether a run has filled `column` for some episode.
    pub(super) fn holds(&self, column: &CheckColumn) -> bool {
        self.filled
            .iter()
            .any(|filled| filled == column.column.as_str())
    }
    /// How many episodes the answers table holds.
    pub(super) fn episodes(&self) -> u64 {
        self.episodes
    }
    /// How many episodes lack a cell in some filled check column, such as
    /// ones appended since the last run.
    pub(super) fn unchecked(&self) -> u64 {
        self.unchecked
    }
    /// Every part `kind`'s column among `columns` holds, as a relation:
    /// `episode_id`, `part` and the check's fields, one row per part of every
    /// episode. `None` when the check has no column, having nothing to
    /// measure.
    pub(super) fn parts(&self, columns: &[CheckColumn], kind: CheckKind) -> Option<String> {
        let column = columns.iter().find(|column| column.kind == kind)?;
        Some(column.parts_in(&self.table))
    }
    /// The rows `sql`, a judgment over the parts, answers, each read as `R`.
    /// A judgment answers a row per part or per finding, never a row per
    /// episode, so an answer cut short is an error.
    pub(super) async fn rows<R: DeserializeOwned>(&self, sql: String) -> Result<Vec<R>, String> {
        let snapshot = self
            .client
            .run_spiral_query(sql, PAGE_ROWS)
            .await
            .map_err(|error| error.to_string())?;
        if snapshot.truncated() {
            return Err(format!(
                "A check's judgment answered more than {PAGE_ROWS} rows"
            ));
        }
        snapshot
            .into_rows()
            .into_iter()
            .map(|row| {
                serde_json::from_value(serde_json::Value::Object(row))
                    .map_err(|error| format!("Reading a check's judgment: {error}"))
            })
            .collect()
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
    fn a_columns_parts_unpack_into_one_row_per_part_of_every_episode() {
        let column = CheckColumn::of(
            CheckKind::Rates,
            PartsStatement::new(&["messages", "first_ns"]).part(
                "/a",
                "SELECT id, 1 AS messages, 2 AS first_ns FROM ta".into(),
            ),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            column.parts_in("\"datasets\".\"folding\".\"answers\""),
            format!(
                "(SELECT a.episode_id AS episode_id, p.part AS part, p.messages AS messages, p.first_ns AS first_ns \
                 FROM \"datasets\".\"folding\".\"answers\" a CROSS JOIN LATERAL UNNEST(a.{}.value) p)",
                column.column
            )
        );
    }
}
