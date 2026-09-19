//! Native array measurements grouped and joined by episode UUID.
use super::plan::{DashboardMetric, SignalAnalysis};
use crate::activities::data::DatasetClient;
use serde::Deserialize;
use serde_json::{Value, json};
use spiral_robotics_common::datasets::DatasetInspection;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecordedArraySignal {
    metric: DashboardMetric,
    topic: TopicSource,
    field: String,
    element_field: Option<String>,
    aggregation: EpisodeAggregation,
    joint: Option<NamedJoint>,
    validity: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TopicSource {
    name: String,
    variant: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NamedJoint {
    name: String,
    metadata: TopicSource,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum EpisodeAggregation {
    Mean,
    Min,
    Max,
    Range,
}
impl EpisodeAggregation {
    fn expression(&self, reading: &str) -> String {
        match self {
            Self::Mean => format!("AVG({reading})"),
            Self::Min => format!("MIN({reading})"),
            Self::Max => format!("MAX({reading})"),
            Self::Range => format!("MAX({reading})-MIN({reading})"),
        }
    }
}
impl TopicSource {
    fn reader(&self, inspection: &DatasetInspection) -> Result<String, String> {
        if !inspection.topics().iter().any(|topic| {
            topic.name() == self.name
                && topic
                    .revision()
                    .is_some_and(|revision| revision.variant() == self.variant)
        }) {
            return Err("Choose an inspected topic and its exact schema variant.".into());
        }
        Ok(format!(
            "spql.mcap.read_topic('{}','{}','{}')",
            inspection
                .dataset()
                .selection()
                .sql_name()
                .replace('\'', "''"),
            self.name.replace('\'', "''"),
            self.variant.replace('\'', "''")
        ))
    }
    fn schema() -> Value {
        json!({"type":"object","properties":{"name":{"type":"string"},"variant":{"type":"string"}},"required":["name","variant"],"additionalProperties":false})
    }
}
impl RecordedArraySignal {
    pub(super) fn definition() -> Value {
        json!({"type":"function","function":{"name":"analyze_array_signal","description":"Reliably measure a discovered array-valued physical signal with native UNNEST SQL, grouped by episode UUID. Prefer this for joint positions, temperature, voltage, load and current. List elements may be numbers or structs containing a numeric field; select element_field from the inspected query schema. Native episode IDs identify readings and joint metadata. Choose the actual field from an inspected schema. A named joint uses inspected metadata value.joint_names.values. Missing measurements are preserved. The query compiler owns joins and arrays; you choose what to measure.","parameters":{"type":"object","properties":{
            "metric":super::agent::ComparisonAgent::metric_schema(),"topic":TopicSource::schema(),
            "field":{"type":"string","description":"Exact array path from inspect_query, e.g. value.position for a numeric list, value.state_position.values for a wrapped list, or value when the topic value itself is a list. Never append values unless it is an actual struct field."},
            "element_field":{"type":["string","null"],"description":"Path inside each list element to its numeric measurement. Use null for List(Float64) or other numeric lists; use value only for List(Struct(value: numeric)). Do not infer wrappers from schema names."},
            "aggregation":{"type":"string","enum":["mean","min","max","range"]},
            "joint":{"type":["object","null"],"properties":{"name":{"type":"string"},"metadata":TopicSource::schema()},"required":["name","metadata"],"additionalProperties":false},
            "validity":{"type":["string","null"],"description":"Actual boolean field path such as value.state_valid, or null for metadata measurements."}
        },"required":["metric","topic","field","element_field","aggregation","joint","validity"],"additionalProperties":false}}})
    }
    pub(super) fn metric(&self) -> &DashboardMetric {
        &self.metric
    }
    fn field(path: &str) -> Result<&str, String> {
        if (path != "value" && !path.starts_with("value.")) || !Self::valid_path(path) {
            return Err("Use an inspected field path rooted at value.".into());
        }
        Ok(path)
    }
    fn valid_path(path: &str) -> bool {
        path.split('.').all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
    }
    pub(super) async fn prepare(
        self,
        inspection: &DatasetInspection,
    ) -> Result<SignalAnalysis, String> {
        if let Some(joint) = &self.joint {
            let names_reader = joint.metadata.reader(inspection)?;
            let duplicates = DatasetClient::current()
                .run_spiral_query(format!("SELECT COUNT(*) AS invalid_count FROM (SELECT id FROM {names_reader} GROUP BY id HAVING COUNT(*)<>1 OR id IS NULL) duplicates"), 1)
                .await.map_err(|error| error.to_string())?;
            if duplicates
                .rows()
                .first()
                .and_then(|row| row.get("invalid_count"))
                .and_then(Value::as_u64)
                != Some(0)
            {
                return Err("Joint-name metadata must have one row per recorded episode.".into());
            }
        }
        let sql = self.compile(inspection)?;
        Ok(SignalAnalysis::new(self.metric, sql))
    }
    fn compile(&self, inspection: &DatasetInspection) -> Result<String, String> {
        let reading = match self.element_field.as_deref() {
            None => "reading".to_owned(),
            Some(path) if Self::valid_path(path) => format!("reading.{path}"),
            Some(_) => {
                return Err(
                    "Use an inspected numeric element field, or null for numeric lists.".into(),
                );
            }
        };
        let reader = self.topic.reader(inspection)?;
        let field = Self::field(&self.field)?;
        let validity = self
            .validity
            .as_deref()
            .map(Self::field)
            .transpose()?
            .map(|field| format!("WHERE {field}=TRUE"))
            .unwrap_or_default();
        let observations =
            format!("SELECT id AS episode_id, {field} AS readings FROM {reader} {validity}");
        let (metadata, expansion, filter): (String, String, String) = match &self.joint {
            None => (
                String::new(),
                "FROM observations o CROSS JOIN LATERAL UNNEST(o.readings) AS reading".into(),
                String::new(),
            ),
            Some(joint) => {
                let names_reader = joint.metadata.reader(inspection)?;
                (format!(", joint_metadata AS (SELECT id AS episode_id,value.joint_names.values AS names FROM {names_reader})"),
                "FROM observations o JOIN joint_metadata j ON o.episode_id=j.episode_id CROSS JOIN LATERAL UNNEST(j.names,o.readings) AS named(joint,reading)".into(),
                format!("WHERE joint.value='{}'",joint.name.replace('\'',"''")))
            }
        };
        Ok(format!(
            "WITH observations AS ({observations}){metadata} SELECT o.episode_id, {} AS value {expansion} {filter} GROUP BY o.episode_id",
            self.aggregation.expression(&reading)
        ))
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use spiral_robotics_common::datasets::{
        DatasetSelection, DatasetSummary, DatasetTopic, TopicRevision,
    };

    fn inspection() -> DatasetInspection {
        let dataset = DatasetSummary::new(
            DatasetSelection::new(vec!["recordings".into()]).unwrap(),
            ("dataset".into(), "v1".into()),
        );
        let topic = DatasetTopic::new("/state".into(), vec!["RobotState".into()])
            .with_revision(TopicRevision::new("v1".into(), "variant".into()));
        DatasetInspection::new(dataset, Some(40), vec![topic])
    }

    fn signal(field: &str, element_field: Option<&str>) -> RecordedArraySignal {
        serde_json::from_value(json!({
            "metric":{"column":"motion","label":"Motion","unit":"recorded","show":true},
            "topic":{"name":"/state","variant":"variant"},
            "field":field,"element_field":element_field,"aggregation":"range",
            "joint":null,"validity":null
        }))
        .unwrap()
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn numeric_lists_do_not_project_struct_wrappers() {
        for field in ["value.position", "value.velocity", "value.torque", "value"] {
            let sql = signal(field, None).compile(&inspection()).unwrap();
            assert!(sql.contains(&format!("{field} AS readings")));
            assert!(sql.contains("MAX(reading)-MIN(reading) AS value"));
            assert!(sql.contains("GROUP BY o.episode_id"));
            assert!(!sql.contains("reading.value"));
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn struct_lists_preserve_validity_and_named_joint_measurements() {
        let mut signal = signal("value.state_position.values", Some("value"));
        signal.validity = Some("value.state_valid".into());
        signal.joint = Some(NamedJoint {
            name: "elbow's joint".into(),
            metadata: TopicSource {
                name: "/state".into(),
                variant: "variant".into(),
            },
        });
        let sql = signal.compile(&inspection()).unwrap();
        assert!(sql.contains("MAX(reading.value)-MIN(reading.value) AS value"));
        assert!(sql.contains("WHERE value.state_valid=TRUE"));
        assert!(sql.contains("ON o.episode_id=j.episode_id"));
        assert!(sql.contains("UNNEST(j.names,o.readings) AS named(joint,reading)"));
        assert!(sql.contains("WHERE joint.value='elbow''s joint'"));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn element_paths_and_topic_variants_are_validated() {
        for path in ["", ".value", "value.", "value); DROP TABLE x", "value--"] {
            assert!(
                signal("value.position", Some(path))
                    .compile(&inspection())
                    .is_err()
            );
        }
        assert!(signal("position", None).compile(&inspection()).is_err());
        let mut signal = signal("value.position", None);
        signal.topic.variant = "uninspected".into();
        assert!(signal.compile(&inspection()).is_err());
    }
}
