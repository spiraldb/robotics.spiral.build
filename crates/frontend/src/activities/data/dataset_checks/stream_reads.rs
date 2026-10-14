//! How the checks read a dataset's recorded streams: which topic a check
//! reads and how, an arm's joints as columns, and the start of each episode a
//! moment is measured from. Every check composes its statement from these,
//! so a topic, a field path or a clock is spelled one way.
use spiral_robotics_common::datasets::{
    DatasetSelection, DatasetStreams, DatasetTopic, RecordedValues, ValueColumns,
};

/// The dataset a check reads and the streams it declares.
#[derive(Clone, Copy)]
pub(super) struct CheckedStreams<'a> {
    dataset: &'a DatasetSelection,
    topics: &'a [DatasetTopic],
}

impl<'a> CheckedStreams<'a> {
    pub(super) fn new(dataset: &'a DatasetSelection, topics: &'a [DatasetTopic]) -> Self {
        Self { dataset, topics }
    }
    pub(super) fn topics(&self) -> &'a [DatasetTopic] {
        self.topics
    }
    /// The table holding one declared stream's messages.
    pub(super) fn read(&self, topic: &DatasetTopic) -> String {
        self.dataset.messages(topic.revision()).to_string()
    }
    /// The table holding the messages of the stream `values` names.
    pub(super) fn read_values(&self, values: &RecordedValues) -> Result<String, String> {
        DatasetStreams::new(self.topics)
            .recorded(values.topic(), values.schema())
            .map(|topic| self.read(topic))
    }
    /// Every episode, joined as `e`, which a moment is measured from.
    pub(super) fn episodes(&self) -> String {
        self.dataset.episodes().to_string()
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

/// A dataset recording two arms, as the joint checks' tests read it: the
/// left arm's six joints, its commands and its gripper, and the right
/// arm's six joints alone. The topics are named as `abc130k_sample` records
/// them, each stream after its topic.
#[cfg(test)]
pub(super) struct TwoArmDataset {
    dataset: DatasetSelection,
    topics: Vec<DatasetTopic>,
    roles: spiral_robotics_common::datasets::DatasetJointRoles,
}

#[cfg(test)]
impl TwoArmDataset {
    pub(super) fn new() -> Self {
        let values = |topic: &str, joints: usize| {
            serde_json::json!({
                "topic": topic, "schema": "JointState", "field": ["position"], "element": [],
                "indices": (0..joints).collect::<Vec<_>>(),
            })
        };
        let roles = serde_json::from_value(serde_json::json!({"arms": [
            {
                "name": "left",
                "state": values("/left-arm-state", 6),
                "action": values("/left-arm-action", 6),
                "gripper": {
                    "state": values("/left-ee-state", 1), "action": values("/left-ee-action", 1),
                    "open": 1.0, "closed": 0.0,
                },
            },
            {"name": "right", "state": values("/right-arm-state", 6), "action": null, "gripper": null},
        ]}))
        .unwrap();
        let topics = [
            "/left-arm-state",
            "/left-arm-action",
            "/left-ee-state",
            "/left-ee-action",
            "/right-arm-state",
        ]
        .map(|topic| Self::stream(topic, "JointState"))
        .into();
        Self {
            dataset: DatasetSelection::new("abc130k_sample").unwrap(),
            topics,
            roles,
        }
    }
    /// The stream of `topic` recorded as `schema`, named after the topic:
    /// `/left-arm-state` is `left_arm_state_00000000`.
    pub(super) fn stream(topic: &str, schema: &str) -> DatasetTopic {
        let stream = format!(
            "{}_00000000",
            topic.trim_start_matches('/').replace(['/', '-', '.'], "_")
        );
        DatasetTopic::new(
            topic.into(),
            spiral_robotics_common::datasets::TopicRevision::new(stream).unwrap(),
            "protobuf".into(),
        )
        .with_schema(Some(schema.into()))
    }
    pub(super) fn streams(&self) -> CheckedStreams<'_> {
        CheckedStreams::new(&self.dataset, &self.topics)
    }
    pub(super) fn arms(&self) -> &[spiral_robotics_common::datasets::ArmRoles] {
        self.roles.arms()
    }
    /// The statement filling `column` over every episode.
    pub(super) fn statement(&self, column: &super::check_columns::CheckColumn) -> String {
        match column.fill(self.streams()).source() {
            spiral_robotics_common::curation::AnswerSource::DatasetStatement { statement } => {
                statement.clone()
            }
            other => panic!("a check column is filled by one dataset statement, not {other:?}"),
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
