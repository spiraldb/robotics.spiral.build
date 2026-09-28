//! How the checks read a dataset's recorded streams: which topic a check
//! reads and how, an arm's joints as columns, and the start of each episode a
//! moment is measured from. Every check composes its statement from these,
//! so a topic, a field path or a clock is spelled one way.
use crate::activities::data::DatasetClient;
use serde::de::DeserializeOwned;
use spiral_robotics_common::datasets::{DatasetSelection, DatasetTopic, RecordedValues};
use spiral_robotics_common::sql::SqlLiteral;

/// The most rows one check statement may answer with; the checks aggregate
/// per episode to stay under it.
const MAX_ROWS: usize = 1000;

/// The dataset a report checks, the streams it declares, and the client
/// its statements run through.
#[derive(Clone, Copy)]
pub(super) struct CheckedStreams<'a> {
    client: &'a DatasetClient,
    dataset: &'a DatasetSelection,
    topics: &'a [DatasetTopic],
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
        match topic.revision() {
            Some(revision) => format!(
                "spql.mcap.read_topic({dataset}, {name}, {})",
                SqlLiteral::text(revision.variant())
            ),
            None => format!("spql.mcap.read_topic({dataset}, {name})"),
        }
    }
    /// The statement reading the stream `values` names.
    pub(super) fn read_values(&self, values: &RecordedValues) -> Result<String, String> {
        self.topics
            .iter()
            .find(|topic| {
                topic.name() == values.topic()
                    && topic
                        .schemas()
                        .iter()
                        .any(|schema| schema == values.schema())
            })
            .map(|topic| self.read(topic))
            .ok_or_else(|| format!("The dataset no longer records {}", values.topic()))
    }
    /// Every episode, joined as `e`, which a moment is measured from.
    pub(super) fn episodes(&self) -> String {
        format!(
            "spql.mcap.episodes({})",
            SqlLiteral::text(&self.dataset.sql_name())
        )
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
    /// Run `sql` and read each row as `R`. A report aggregates per episode,
    /// so an answer cut short is an error rather than a smaller report.
    pub(super) async fn rows<R: DeserializeOwned>(&self, sql: String) -> Result<Vec<R>, String> {
        let snapshot = self
            .client
            .run_spiral_query(sql, MAX_ROWS)
            .await
            .map_err(|error| error.to_string())?;
        if snapshot.truncated() {
            return Err(format!(
                "A check answered with more than {MAX_ROWS} rows; this dataset has more episodes than the report reads"
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
    /// Run `parts`, statements answering at most a row per episode each
    /// with the same columns, in as few statements as the row limit allows,
    /// and read each row as `R` beside the position of the part it answers.
    pub(super) async fn parts<R: DeserializeOwned>(
        &self,
        parts: &[String],
    ) -> Result<Vec<(usize, R)>, String> {
        let per_statement = (MAX_ROWS as u64 / self.episode_count().max(1)).max(1) as usize;
        let mut answered = Vec::new();
        for (chunk, statements) in parts.chunks(per_statement).enumerate() {
            let sql = statements
                .iter()
                .enumerate()
                .map(|(part, sql)| format!("SELECT {part} AS part, * FROM ({sql}) x"))
                .collect::<Vec<_>>()
                .join(" UNION ALL ");
            let rows: Vec<PartRow<R>> = self.rows(sql).await?;
            answered.extend(
                rows.into_iter()
                    .map(|tagged| (chunk * per_statement + tagged.part, tagged.row)),
            );
        }
        Ok(answered)
    }
}

/// One row of a part, and which part it answers.
#[derive(serde::Deserialize)]
struct PartRow<R> {
    part: usize,
    #[serde(flatten)]
    row: R,
}

/// Numbers of one stream as columns: `{prefix}0`, `{prefix}1`, … per
/// message, beside the episode `id` and `log_time`.
pub(super) struct ValueColumns<'a> {
    values: &'a RecordedValues,
    prefix: &'static str,
}

impl<'a> ValueColumns<'a> {
    pub(super) fn new(values: &'a RecordedValues, prefix: &'static str) -> Self {
        Self { values, prefix }
    }
    /// How many numbers each message holds.
    pub(super) fn len(&self) -> usize {
        self.values.indices().len().max(1)
    }
    pub(super) fn names(&self) -> Vec<String> {
        (0..self.len())
            .map(|position| format!("{}{position}", self.prefix))
            .collect()
    }
    /// The column expressions picking each number from `value`, as the
    /// statement `read` answers it. A list item's element is a field of a
    /// picked column, which Spiral reads only from a named column, so an
    /// element path takes a second step.
    pub(super) fn ctes(&self, name: &str, read: &str) -> Vec<String> {
        let field = Self::path("value", self.values.field());
        let names = self.names();
        if self.values.indices().is_empty() {
            return vec![format!(
                "{name} AS (SELECT id, log_time, {field} AS {} FROM {read})",
                names[0]
            )];
        }
        let picked = self
            .values
            .indices()
            .iter()
            .zip(&names)
            .map(|(index, column)| format!("spql.list.get({field}, {index}) AS {column}"))
            .collect::<Vec<_>>()
            .join(", ");
        if self.values.element().is_empty() {
            return vec![format!(
                "{name} AS (SELECT id, log_time, {picked} FROM {read})"
            )];
        }
        let elements = names
            .iter()
            .map(|column| format!("{} AS {column}", Self::path(column, self.values.element())))
            .collect::<Vec<_>>()
            .join(", ");
        vec![
            format!("{name}_items AS (SELECT id, log_time, {picked} FROM {read})"),
            format!("{name} AS (SELECT id, log_time, {elements} FROM {name}_items)"),
        ]
    }
    /// `{name}_motion`, over `{name}`: each message's numbers beside the
    /// seconds since the episode's previous message (`dt`) and how fast each
    /// number changed over them (`v0`, `v1`, …).
    pub(super) fn motion_cte(&self, name: &str) -> String {
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
    pub(super) fn joint_name(position: usize) -> String {
        format!("j{}", position + 1)
    }
    /// `root` then each of `names`, quoted, as one field path.
    fn path(root: &str, names: &[String]) -> String {
        std::iter::once(root)
            .chain(names.iter().map(String::as_str))
            .map(|name| format!("\"{}\"", name.replace('"', "\"\"")))
            .collect::<Vec<_>>()
            .join(".")
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
        let state = streams.read_values(self.state.values)?;
        let action = streams.read_values(self.action.values)?;
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
    use serde_json::json;

    fn values(value: serde_json::Value) -> RecordedValues {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn list_items_with_an_element_take_a_named_column_first() {
        let wrapped = values(json!({
            "topic": "/joints", "schema": "JointSample", "field": ["state_position", "values"],
            "element": ["value"], "indices": [0, 4]
        }));
        let ctes = ValueColumns::new(&wrapped, "q").ctes("follower", "READ");
        assert_eq!(
            ctes,
            [
                "follower_items AS (SELECT id, log_time, spql.list.get(\"value\".\"state_position\".\"values\", 0) AS q0, spql.list.get(\"value\".\"state_position\".\"values\", 4) AS q1 FROM READ)",
                "follower AS (SELECT id, log_time, \"q0\".\"value\" AS q0, \"q1\".\"value\" AS q1 FROM follower_items)",
            ]
        );
        let plain = values(json!({
            "topic": "/arm", "schema": "RobotState", "field": ["position"], "element": [], "indices": [2]
        }));
        assert_eq!(
            ValueColumns::new(&plain, "a").ctes("arm", "READ"),
            [
                "arm AS (SELECT id, log_time, spql.list.get(\"value\".\"position\", 2) AS a0 FROM READ)"
            ]
        );
        let scalar = values(json!({
            "topic": "/grip", "schema": "Grip", "field": ["opening"], "element": [], "indices": []
        }));
        assert_eq!(
            ValueColumns::new(&scalar, "q").ctes("grip", "READ"),
            ["grip AS (SELECT id, log_time, \"value\".\"opening\" AS q0 FROM READ)"]
        );
        assert_eq!(Greatest::of(&["x".into()]), "x");
        assert_eq!(Greatest::of(&["x".into(), "y".into()]), "greatest(x, y)");
    }
}
