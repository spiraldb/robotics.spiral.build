//! Numbers one recorded stream holds, read as columns: how a dataset check
//! measures an arm's joints and how training reads its state and action.
use super::RecordedValues;
use crate::sql::{SqlList, SqlLiteral};

/// Numbers of one stream as columns: `{prefix}0`, `{prefix}1`, … per
/// message, beside the episode `id` and `log_time`.
pub struct ValueColumns<'a> {
    values: &'a RecordedValues,
    prefix: &'static str,
}

impl<'a> ValueColumns<'a> {
    pub fn new(values: &'a RecordedValues, prefix: &'static str) -> Self {
        Self { values, prefix }
    }
    /// The recorded values these columns read.
    pub fn values(&self) -> &'a RecordedValues {
        self.values
    }
    /// How many numbers each message holds.
    pub fn count(&self) -> usize {
        self.values.count()
    }
    pub fn names(&self) -> Vec<String> {
        (0..self.count())
            .map(|position| format!("{}{position}", self.prefix))
            .collect()
    }
    /// The entry `name` picking each number from `value`, one column each,
    /// as the statement `read` answers it: a member or a field of `value`, or
    /// an item of a list there, and the item's element when it is a struct.
    pub fn cte(&self, name: &str, read: &str) -> String {
        let field = Self::path("value", self.values.field());
        let picked = if !self.values.members().is_empty() {
            self.values
                .members()
                .iter()
                .map(|member| {
                    let path = self.values.field().iter().chain([member]).cloned();
                    Self::path("value", &path.collect::<Vec<_>>())
                })
                .collect::<Vec<_>>()
        } else if self.values.indices().is_empty() {
            vec![field]
        } else {
            self.values
                .indices()
                .iter()
                .map(|index| {
                    let item = SqlList::item_at(&field, *index);
                    self.values.element().iter().fold(item, |picked, name| {
                        format!("get_field({picked}, {})", SqlLiteral::text(name))
                    })
                })
                .collect()
        };
        let columns = picked
            .iter()
            .zip(self.names())
            .map(|(picked, column)| format!("{picked} AS {column}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{name} AS (SELECT id, log_time, {columns} FROM {read})")
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn values(value: serde_json::Value) -> RecordedValues {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn list_items_are_picked_by_position_and_their_element_by_name() {
        let wrapped = values(json!({
            "topic": "/joints", "schema": "JointSample", "field": ["state_position", "values"],
            "element": ["value"], "indices": [0, 4]
        }));
        assert_eq!(
            ValueColumns::new(&wrapped, "q").cte("follower", "READ"),
            "follower AS (SELECT id, log_time, get_field(array_element(\"value\".\"state_position\".\"values\", 1), 'value') AS q0, get_field(array_element(\"value\".\"state_position\".\"values\", 5), 'value') AS q1 FROM READ)"
        );
        let plain = values(json!({
            "topic": "/arm", "schema": "RobotState", "field": ["position"], "element": [], "indices": [2]
        }));
        assert_eq!(
            ValueColumns::new(&plain, "a").cte("arm", "READ"),
            "arm AS (SELECT id, log_time, array_element(\"value\".\"position\", 3) AS a0 FROM READ)"
        );
        let scalar = values(json!({
            "topic": "/grip", "schema": "Grip", "field": ["opening"], "element": [], "indices": []
        }));
        assert_eq!(
            ValueColumns::new(&scalar, "q").cte("grip", "READ"),
            "grip AS (SELECT id, log_time, \"value\".\"opening\" AS q0 FROM READ)"
        );
    }

    #[test]
    fn named_members_read_as_one_column_each_however_they_are_spelled() {
        let named = values(json!({
            "topic": "observation.state", "schema": null, "field": [], "element": [], "indices": [],
            "members": ["shoulder_pan.pos", "left_joint1"]
        }));
        let columns = ValueColumns::new(&named, "q");
        assert_eq!(columns.count(), 2);
        assert_eq!(
            columns.cte("state", "READ"),
            "state AS (SELECT id, log_time, \"value\".\"shoulder_pan.pos\" AS q0, \"value\".\"left_joint1\" AS q1 FROM READ)"
        );
    }
}
