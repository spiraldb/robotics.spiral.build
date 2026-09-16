//! Native array measurements grouped and joined by episode UUID.
use crate::activities::data::DatasetClient;
use serde::Deserialize;
use serde_json::{Value, json};
use spiral_robotics_common::{
    datasets::DatasetInspection,
    workflow::comparison_plan::{DashboardMetric, SignalAnalysis},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecordedArraySignal {
    metric: DashboardMetric,
    topic: TopicSource,
    field: String,
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
    fn expression(&self) -> &'static str {
        match self {
            Self::Mean => "AVG(reading.value)",
            Self::Min => "MIN(reading.value)",
            Self::Max => "MAX(reading.value)",
            Self::Range => "MAX(reading.value)-MIN(reading.value)",
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
        json!({"type":"function","function":{"name":"analyze_array_signal","description":"Reliably measure a discovered array-valued physical signal with native UNNEST SQL, grouped by episode UUID. Prefer this for joint positions, temperature, voltage, load and current. Each list element must be a struct with numeric value. Native episode IDs identify readings and joint metadata. Choose the actual field from an inspected schema. A named joint uses inspected metadata value.joint_names.values. Missing measurements are preserved. The query compiler owns joins and arrays; you choose what to measure.","parameters":{"type":"object","properties":{
            "metric":super::agent::ComparisonAgent::metric_schema(),"topic":TopicSource::schema(),
            "field":{"type":"string","description":"Actual array field path including value, e.g. value.peak_temperature_celsius.values or value.state_position.values."},
            "aggregation":{"type":"string","enum":["mean","min","max","range"]},
            "joint":{"type":["object","null"],"properties":{"name":{"type":"string"},"metadata":TopicSource::schema()},"required":["name","metadata"],"additionalProperties":false},
            "validity":{"type":["string","null"],"description":"Actual boolean field path such as value.state_valid, or null for metadata measurements."}
        },"required":["metric","topic","field","aggregation","joint","validity"],"additionalProperties":false}}})
    }
    pub(super) fn metric(&self) -> &DashboardMetric {
        &self.metric
    }
    fn field(path: &str) -> Result<&str, String> {
        if !path.starts_with("value.")
            || !path.split('.').all(|part| {
                !part.is_empty()
                    && part
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '_')
            })
        {
            return Err("Use an inspected field path starting with value.".into());
        }
        Ok(path)
    }
    pub(super) async fn prepare(
        self,
        inspection: &DatasetInspection,
    ) -> Result<SignalAnalysis, String> {
        let data = DatasetClient::current();
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
                let duplicates=data.run_spiral_query(format!("SELECT COUNT(*) AS invalid_count FROM (SELECT id FROM {names_reader} GROUP BY id HAVING COUNT(*)<>1 OR id IS NULL) duplicates"),1).await.map_err(|error|error.to_string())?;
                if duplicates
                    .rows()
                    .first()
                    .and_then(|row| row.get("invalid_count"))
                    .and_then(Value::as_u64)
                    != Some(0)
                {
                    return Err(
                        "Joint-name metadata must have one row per recorded episode.".into(),
                    );
                }
                (format!(", joint_metadata AS (SELECT id AS episode_id,value.joint_names.values AS names FROM {names_reader})"),
                "FROM observations o JOIN joint_metadata j ON o.episode_id=j.episode_id CROSS JOIN LATERAL UNNEST(j.names,o.readings) AS named(joint,reading)".into(),
                format!("WHERE joint.value='{}'",joint.name.replace('\'',"''")))
            }
        };
        let sql = format!(
            "WITH observations AS ({observations}){metadata} SELECT o.episode_id, {} AS value {expansion} {filter} GROUP BY o.episode_id",
            self.aggregation.expression()
        );
        Ok(SignalAnalysis::new(self.metric, sql))
    }
}
