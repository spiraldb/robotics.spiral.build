//! Numbers one recorded stream holds, read as columns: how a dataset check
//! measures an arm's joints and how training reads its state and action.
use super::RecordedValues;

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
        self.values.indices().len().max(1)
    }
    pub fn names(&self) -> Vec<String> {
        (0..self.count())
            .map(|position| format!("{}{position}", self.prefix))
            .collect()
    }
    /// The column expressions picking each number from `value`, as the
    /// statement `read` answers it. A list item's element is a field of a
    /// picked column, which Spiral reads only from a named column, so an
    /// element path takes a second step.
    pub fn ctes(&self, name: &str, read: &str) -> Vec<String> {
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
    }
}
