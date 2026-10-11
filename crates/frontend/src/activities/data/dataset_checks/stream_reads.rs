//! How the checks read a dataset's recorded streams: which topic a check
//! reads and how, an arm's joints as columns, and the start of each episode a
//! moment is measured from. Every check composes its statement from these,
//! so a topic, a field path or a clock is spelled one way.
use crate::activities::data::DatasetClient;
use serde::de::DeserializeOwned;
use spiral_robotics_common::datasets::{
    DatasetSelection, DatasetTopic, RecordedValues, ValueColumns,
};
use spiral_robotics_common::sql::SqlLiteral;

/// The most rows one check statement may answer with; the checks aggregate
/// per episode, and a dataset with more episodes is read in chunks of them.
const MAX_ROWS: usize = 1000;

/// The dataset a report checks, the streams it declares, the client its
/// statements run through, and the chunk of its episodes a statement covers.
#[derive(Clone, Copy)]
pub(super) struct CheckedStreams<'a> {
    client: &'a DatasetClient,
    dataset: &'a DatasetSelection,
    topics: &'a [DatasetTopic],
    chunk: Option<EpisodeChunk>,
}

/// The consecutive episodes, in UUID order, one statement covers: those
/// after the first `after`, through the `through`-th or to the last.
#[derive(Clone, Copy)]
struct EpisodeChunk {
    after: u64,
    through: Option<u64>,
}

impl<'a> CheckedStreams<'a> {
    pub(super) fn new(
        client: &'a DatasetClient,
        dataset: &'a DatasetSelection,
        topics: &'a [DatasetTopic],
    ) -> Self {
        Self {
            client,
            dataset,
            topics,
            chunk: None,
        }
    }
    pub(super) fn topics(&self) -> &'a [DatasetTopic] {
        self.topics
    }
    /// How many episodes the dataset holds: as many as its most widely
    /// recorded stream appears in.
    pub(super) fn episode_count(&self) -> u64 {
        self.topics
            .iter()
            .map(DatasetTopic::members)
            .max()
            .unwrap_or_default()
    }
    /// The statement reading one declared topic.
    pub(super) fn read(&self, topic: &DatasetTopic) -> String {
        let dataset = SqlLiteral::text(&self.dataset.sql_name());
        let name = SqlLiteral::text(topic.name());
        self.in_chunk(match topic.revision() {
            Some(revision) => format!(
                "spql.mcap.read_topic({dataset}, {name}, {})",
                SqlLiteral::text(revision.variant())
            ),
            None => format!("spql.mcap.read_topic({dataset}, {name})"),
        })
    }
    /// The statement reading the stream `values` names.
    pub(super) fn read_values(&self, values: &RecordedValues) -> Result<String, String> {
        self.topics
            .iter()
            .find(|topic| topic.is_recorded_as(values.topic(), values.schema()))
            .map(|topic| self.read(topic))
            .ok_or_else(|| format!("The dataset no longer records {}", values.topic()))
    }
    /// Every episode, joined as `e`, which a moment is measured from.
    pub(super) fn episodes(&self) -> String {
        self.in_chunk(self.all_episodes())
    }
    fn all_episodes(&self) -> String {
        format!(
            "spql.mcap.episodes({})",
            SqlLiteral::text(&self.dataset.sql_name())
        )
    }
    /// `relation` cut to this chunk's episodes, when a statement covers one.
    /// Spiral reads only the episodes an `id IN` query selects.
    fn in_chunk(&self, relation: String) -> String {
        let Some(chunk) = self.chunk else {
            return relation;
        };
        let through = chunk
            .through
            .map(|through| format!(" AND position <= {through}"))
            .unwrap_or_default();
        format!(
            "(SELECT * FROM {relation} WHERE id IN (SELECT id FROM (SELECT id, ROW_NUMBER() OVER (ORDER BY id) AS position FROM {episodes}) WHERE position > {after}{through}))",
            episodes = self.all_episodes(),
            after = chunk.after,
        )
    }
    /// These streams in chunks of `size` episodes, or whole when the dataset
    /// holds no more than that.
    fn chunks(self, size: u64) -> Vec<Self> {
        let episodes = self.episode_count();
        if episodes <= size {
            return vec![self];
        }
        (0..episodes)
            .step_by(size as usize)
            .map(|after| Self {
                chunk: Some(EpisodeChunk {
                    after,
                    through: (after + size < episodes).then_some(after + size),
                }),
                ..self
            })
            .collect()
    }
    /// Seconds from the start of episode `e` to the nanosecond `time`: the
    /// instant an episode's player seeks to.
    pub(super) fn since_start(time: &str) -> String {
        format!("(CAST({time} AS BIGINT) - CAST(e.message_start_time AS BIGINT)) / 1e9")
    }
    /// Seconds since the previous message of the same episode.
    pub(super) fn step_seconds() -> &'static str {
        "(CAST(log_time AS BIGINT) - CAST(LAG(log_time) OVER (PARTITION BY id ORDER BY log_time) AS BIGINT)) / 1e9"
    }
    /// The rows of the statement `build` makes, at most one per episode,
    /// each read as `R`. A dataset with more episodes than one answer may
    /// hold is read a chunk of them at a time.
    pub(super) async fn rows<R: DeserializeOwned>(
        &self,
        build: impl Fn(CheckedStreams<'a>) -> Result<String, String>,
    ) -> Result<Vec<R>, String> {
        let mut rows = Vec::new();
        for streams in self.chunks(MAX_ROWS as u64) {
            rows.extend(streams.answer(build(streams)?).await?);
        }
        Ok(rows)
    }
    /// Run the statements `build` makes, answering at most a row per
    /// episode each with the same columns, in as few statements as the row
    /// limit allows, a chunk of episodes at a time when the dataset holds
    /// more than one answer may, and read each row as `R` beside the
    /// position of the part it answers.
    pub(super) async fn parts<R: DeserializeOwned>(
        &self,
        build: impl Fn(CheckedStreams<'a>) -> Result<Vec<String>, String>,
    ) -> Result<Vec<(usize, R)>, String> {
        let episodes = self.episode_count().clamp(1, MAX_ROWS as u64);
        let per_statement = (MAX_ROWS as u64 / episodes) as usize;
        let mut answered = Vec::new();
        for streams in self.chunks(episodes) {
            answered.extend(streams.union(&build(streams)?, per_statement).await?);
        }
        Ok(answered)
    }
    /// Run `parts`, statements over every episode answering one row each,
    /// and read each row as `R` beside the position of the part it answers.
    pub(super) async fn dataset_parts<R: DeserializeOwned>(
        &self,
        parts: &[String],
    ) -> Result<Vec<(usize, R)>, String> {
        self.union(parts, MAX_ROWS).await
    }
    /// Run `parts`, `per_statement` of them a statement, tagging each row
    /// with the position of the part it answers.
    async fn union<R: DeserializeOwned>(
        &self,
        parts: &[String],
        per_statement: usize,
    ) -> Result<Vec<(usize, R)>, String> {
        let mut answered = Vec::new();
        for (chunk, statements) in parts.chunks(per_statement).enumerate() {
            let sql = statements
                .iter()
                .enumerate()
                .map(|(part, sql)| format!("SELECT {part} AS part, * FROM ({sql}) x"))
                .collect::<Vec<_>>()
                .join(" UNION ALL ");
            let rows: Vec<PartRow<R>> = self.answer(sql).await?;
            answered.extend(
                rows.into_iter()
                    .map(|tagged| (chunk * per_statement + tagged.part, tagged.row)),
            );
        }
        Ok(answered)
    }
    /// Run `sql` and read each row as `R`. A report aggregates per episode,
    /// so an answer cut short is an error rather than a smaller report.
    async fn answer<R: DeserializeOwned>(&self, sql: String) -> Result<Vec<R>, String> {
        let snapshot = self
            .client
            .run_spiral_query(sql, MAX_ROWS)
            .await
            .map_err(|error| error.to_string())?;
        if snapshot.truncated() {
            return Err(format!(
                "A check answered with more than {MAX_ROWS} rows for one chunk of episodes"
            ));
        }
        snapshot
            .into_rows()
            .into_iter()
            .map(|row| {
                serde_json::from_value(serde_json::Value::Object(row))
                    .map_err(|error| format!("Reading a check's answer: {error}"))
            })
            .collect()
    }
}

/// One row of a part, and which part it answers.
#[derive(serde::Deserialize)]
struct PartRow<R> {
    part: usize,
    #[serde(flatten)]
    row: R,
}

/// What the checks read from a stream's value columns beyond the numbers
/// themselves: how fast they change, and what a position is called.
pub(super) trait CheckedValueColumns {
    fn motion_cte(&self, name: &str) -> String;
    fn joint_name(position: usize) -> String;
}

impl CheckedValueColumns for ValueColumns<'_> {
    /// `{name}_motion`, over `{name}`: each message's numbers beside the
    /// seconds since the episode's previous message (`dt`) and how fast each
    /// number changed over them (`v0`, `v1`, …).
    fn motion_cte(&self, name: &str) -> String {
        let step = CheckedStreams::step_seconds();
        let columns = self.names();
        let rates = columns
            .iter()
            .enumerate()
            .map(|(position, column)| {
                format!(
                    "({column} - LAG({column}) OVER (PARTITION BY id ORDER BY log_time)) / ({step}) AS v{position}"
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{name}_motion AS (SELECT id, log_time, {}, {step} AS dt, {rates} FROM {name})",
            columns.join(", ")
        )
    }
    /// The joint an arm's position `position` is: j1, j2, …
    fn joint_name(position: usize) -> String {
        format!("j{}", position + 1)
    }
}

/// A stream's measured numbers beside the commanded ones logged at the same
/// instants: `q0…` and `a0…` per paired message, from one message when both
/// are recorded in it, or joined on the instant when they are two streams.
/// `{name}_commands` counts the commanded messages of each episode, so a
/// check can tell an episode whose streams were not logged together.
pub(super) struct PairedColumns<'a> {
    state: ValueColumns<'a>,
    action: ValueColumns<'a>,
}

impl<'a> PairedColumns<'a> {
    pub(super) fn new(state: &'a RecordedValues, action: &'a RecordedValues) -> Self {
        Self {
            state: ValueColumns::new(state, "q"),
            action: ValueColumns::new(action, "a"),
        }
    }
    pub(super) fn state(&self) -> &ValueColumns<'a> {
        &self.state
    }
    pub(super) fn ctes(
        &self,
        name: &str,
        streams: CheckedStreams<'_>,
    ) -> Result<Vec<String>, String> {
        let state = streams.read_values(self.state.values())?;
        let action = streams.read_values(self.action.values())?;
        let mut ctes = self.state.ctes(&format!("{name}_state"), &state);
        ctes.extend(self.action.ctes(&format!("{name}_action"), &action));
        let measured = self
            .state
            .names()
            .iter()
            .map(|column| format!("s.{column} AS {column}"))
            .chain(
                self.action
                    .names()
                    .iter()
                    .map(|column| format!("c.{column} AS {column}")),
            )
            .collect::<Vec<_>>()
            .join(", ");
        ctes.push(format!(
            "{name} AS (SELECT c.id AS id, c.log_time AS log_time, {measured} FROM {name}_action c JOIN {name}_state s ON s.id = c.id AND s.log_time = c.log_time)"
        ));
        ctes.push(format!(
            "{name}_commands AS (SELECT id, count(*) AS commands FROM {name}_action GROUP BY id)"
        ));
        Ok(ctes)
    }
}

/// The largest of `terms`, which Spiral spells only for two or more.
pub(super) struct Greatest;

impl Greatest {
    pub(super) fn of(terms: &[String]) -> String {
        match terms {
            [one] => one.clone(),
            _ => format!("greatest({})", terms.join(", ")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greatest_is_spelled_only_for_two_or_more() {
        assert_eq!(Greatest::of(&["x".into()]), "x");
        assert_eq!(Greatest::of(&["x".into(), "y".into()]), "greatest(x, y)");
    }
}
