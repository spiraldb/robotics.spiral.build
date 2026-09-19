//! Comparison-owned interpretation of two unmodified workflow queries.
use serde::{Deserialize, Serialize};
use spiral_robotics_common::{
    datasets::DatasetSelection,
    sql::{ReadOnlyQuery, ReadOnlyQueryMode},
};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ComparisonPlan {
    title: String,
    summary: String,
    dataset: Option<DatasetSelection>,
    unit: ObservationUnit,
    queries: ComparisonQueries,
    metrics: Vec<DashboardMetric>,
    #[serde(skip)]
    measurement: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ObservationUnit {
    Episodes,
    Rows,
}
impl ObservationUnit {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Episodes => "episodes",
            Self::Rows => "rows",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ComparisonQueries {
    left: String,
    right: String,
}
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DashboardMetric {
    column: String,
    label: String,
    unit: String,
    show: bool,
    #[serde(default)]
    description: String,
    #[serde(default)]
    source: String,
}
impl DashboardMetric {
    pub(super) fn description(&self) -> &str {
        &self.description
    }
    pub(super) fn source(&self) -> &str {
        &self.source
    }
    pub(super) fn column(&self) -> &str {
        &self.column
    }
    pub(super) fn label(&self) -> &str {
        &self.label
    }
    pub(super) fn unit(&self) -> &str {
        &self.unit
    }
    pub(super) fn shown(&self) -> bool {
        self.show
    }
}
impl ComparisonPlan {
    /// Enrich the already chosen populations with one measured value per episode.
    /// LEFT JOIN keeps episodes with missing measurements in the population.
    pub(super) fn with_measurement(&self, analysis: SignalAnalysis) -> Result<Self, String> {
        if self.unit != ObservationUnit::Episodes || self.dataset.is_none() {
            return Err(
                "Recorded-signal analysis requires episode populations and a dataset.".into(),
            );
        }
        if analysis.metric.description.is_empty() || analysis.metric.source.is_empty() {
            return Err("Describe the per-episode measurement and its recorded source.".into());
        }
        ReadOnlyQuery::new(&analysis.sql, ReadOnlyQueryMode::Workflow)
            .authorize()
            .map_err(|error| error.to_string())?;
        let project = |cohort: &str| {
            format!(
                "SELECT population.episode_id, measurement.value AS \"{}\" FROM ({}) population LEFT JOIN comparison_measurements measurement ON population.episode_id = measurement.episode_id",
                analysis.metric.column,
                cohort.trim().trim_end_matches(';')
            )
        };
        let plan = Self {
            title: self.title.clone(),
            summary: self.summary.clone(),
            dataset: self.dataset.clone(),
            unit: self.unit,
            queries: ComparisonQueries {
                left: project(&self.queries.left),
                right: project(&self.queries.right),
            },
            metrics: vec![analysis.metric],
            measurement: Some(analysis.sql),
        };
        plan.validate()?;
        Ok(plan)
    }
    pub(super) fn dataset(&self) -> Option<&DatasetSelection> {
        self.dataset.as_ref()
    }
    pub(super) fn unit(&self) -> ObservationUnit {
        self.unit
    }
    pub(super) fn metrics(&self) -> &[DashboardMetric] {
        &self.metrics
    }
    pub(super) fn validate(&self) -> Result<(), String> {
        if self.title.is_empty() || self.title.len() > 160 || self.summary.len() > 600 {
            return Err("Use a short dashboard title and summary.".into());
        }
        if self.metrics.is_empty()
            || self.metrics.len() > 8
            || (self.measurement.is_none() && !self.metrics.iter().any(|m| m.show))
        {
            return Err("Choose 1–8 compatible metrics and show at least one.".into());
        }
        let mut columns = std::collections::BTreeSet::new();
        for metric in &self.metrics {
            if metric.column.is_empty()
                || !metric
                    .column
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                || !columns.insert(&metric.column)
                || metric.label.is_empty()
                || metric.label.len() > 120
                || metric.description.len() > 300
                || metric.source.len() > 200
                || metric.unit.len() > 24
            {
                return Err("Metrics need unique simple column aliases, labels, and units.".into());
            }
        }
        if let Some(dataset) = &self.dataset {
            DatasetSelection::new(dataset.path().to_vec())?;
        }
        for query in [&self.queries.left, &self.queries.right] {
            ReadOnlyQuery::new(query, ReadOnlyQueryMode::Workflow)
                .authorize()
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    /// One shared range per metric, computed over both full result populations.
    /// Source queries are embedded verbatim, never rewritten by the agent.
    pub(super) fn aggregate_sql(&self, sources: (&str, Option<&str>)) -> Result<String, String> {
        let prefix = self.query_prefix(sources)?;
        let normalized = self
            .metrics
            .iter()
            .map(|m| format!(", CAST(\"{}\" AS DOUBLE) AS \"{}\"", m.column, m.column))
            .collect::<String>();
        let identity = match self.unit {
            ObservationUnit::Episodes => "episode_id",
            ObservationUnit::Rows => "CAST(NULL AS VARCHAR) AS episode_id",
        };
        let values = self
            .metrics
            .iter()
            .enumerate()
            .map(|(i, m)| {
                format!(
                    "SELECT {i} AS metric, side, \"{}\" AS value FROM comparison_population",
                    m.column
                )
            })
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        let invalid = match self.unit {
            ObservationUnit::Episodes => {
                "COUNT(*) - COUNT(DISTINCT CASE WHEN side = 0 THEN episode_id END) - COUNT(DISTINCT CASE WHEN side = 1 THEN episode_id END)"
            }
            ObservationUnit::Rows => "0",
        };
        let sql = format!(
            r#"{prefix}, comparison_population AS (
 SELECT 0 AS side, {identity}{normalized} FROM normalized_left
 UNION ALL SELECT 1 AS side, {identity}{normalized} FROM normalized_right
), comparison_values AS ({values}),
comparison_ranges AS (
 SELECT metric, MIN(value) AS low, MAX(value) AS high FROM comparison_values
 WHERE value BETWEEN -1.7976931348623157e308 AND 1.7976931348623157e308 GROUP BY metric
), comparison_bins AS (
 SELECT v.*, r.low, r.high, CASE WHEN r.high = r.low THEN 0 WHEN value = r.high THEN 9
 ELSE CAST(spql.math.floor((value-r.low)/(r.high-r.low)*10) AS BIGINT) END AS bucket
 FROM comparison_values v JOIN comparison_ranges r ON v.metric=r.metric
 WHERE value BETWEEN -1.7976931348623157e308 AND 1.7976931348623157e308
)
SELECT -1 AS metric, -1 AS bucket, CAST(NULL AS DOUBLE) AS low, CAST(NULL AS DOUBLE) AS high,
 COUNT(CASE WHEN side=1 THEN 1 END) AS right_n, COUNT(CASE WHEN side=0 THEN 1 END) AS left_n,
 CAST(NULL AS DOUBLE) AS right_sum, CAST(NULL AS DOUBLE) AS left_sum,
 {invalid} AS invalid_count
FROM comparison_population
UNION ALL
SELECT metric, bucket, low, high, COUNT(CASE WHEN side=1 THEN 1 END), COUNT(CASE WHEN side=0 THEN 1 END),
 SUM(CASE WHEN side=1 THEN value END), SUM(CASE WHEN side=0 THEN value END), 0
FROM comparison_bins GROUP BY metric,bucket,low,high ORDER BY metric,bucket"#,
        );
        ReadOnlyQuery::new(&sql, ReadOnlyQueryMode::Workflow)
            .authorize()
            .map_err(|e| e.to_string())?;
        Ok(sql)
    }
    fn query_prefix(&self, sources: (&str, Option<&str>)) -> Result<String, String> {
        self.validate()?;
        for sql in [Some(sources.0), sources.1].into_iter().flatten() {
            ReadOnlyQuery::new(sql, ReadOnlyQueryMode::Workflow)
                .authorize()
                .map_err(|e| e.to_string())?;
        }
        if sources.1.is_none() && self.dataset.is_none() {
            return Err("The all-episodes baseline requires an identifiable dataset.".into());
        }
        let catalog = self.dataset.as_ref().map(|dataset| format!(
            ", episode_catalog AS (SELECT id AS episode_id, CAST(duration_ns AS DOUBLE)/1000000000.0 AS duration_seconds, CAST(message_count AS DOUBLE) AS message_count FROM spql.mcap.episodes('{}'))",
            dataset.sql_name().replace('\'', "''"))).unwrap_or_default();
        let right = sources
            .1
            .map(|sql| sql.trim().trim_end_matches(';'))
            .unwrap_or("SELECT * FROM episode_catalog");
        let measurement = self
            .measurement
            .as_ref()
            .map(|sql| {
                format!(
                    ", comparison_measurements AS ({})",
                    sql.trim().trim_end_matches(';')
                )
            })
            .unwrap_or_default();
        Ok(format!(
            "WITH workflow_left AS ({}){catalog}, workflow_right AS ({right}){measurement}, normalized_left AS ({}), normalized_right AS ({})",
            sources.0.trim().trim_end_matches(';'),
            self.queries.left.trim().trim_end_matches(';'),
            self.queries.right.trim().trim_end_matches(';')
        ))
    }
    pub(super) fn evidence_sql(
        &self,
        sources: (&str, Option<&str>),
        slice: &ComparisonSlice,
    ) -> Result<String, String> {
        let dataset = self
            .dataset
            .as_ref()
            .ok_or("Episode inspection requires a dataset.")?;
        if self.unit != ObservationUnit::Episodes {
            return Err("This comparison contains rows rather than episodes.".into());
        }
        let prefix = self.query_prefix(sources)?;
        let population = if slice.right {
            "normalized_right"
        } else {
            "normalized_left"
        };
        let (value, filter) = match slice.bin {
            Some((index, low, high, last)) => {
                let metric = self.metrics.get(index).ok_or("Unknown metric.")?;
                if !low.is_finite() || !high.is_finite() || low > high {
                    return Err("Invalid metric range.".into());
                }
                let value = format!("CAST(n.\"{}\" AS DOUBLE)", metric.column);
                let end = if last { "<=" } else { "<" };
                (
                    value.clone(),
                    format!("WHERE {value} >= {low} AND {value} {end} {high}"),
                )
            }
            None => ("CAST(NULL AS DOUBLE)".into(), String::new()),
        };
        Ok(format!(
            "{prefix} SELECT e.id AS episode_id, e.source.uri AS source_uri, e.message_start_time, e.message_end_time, e.duration_ns, e.message_count, CAST(e.duration_ns AS DOUBLE)/1000000000.0 AS duration_seconds, {value} AS criterion_value, 1 AS cohort FROM {population} n JOIN spql.mcap.episodes('{}') e ON n.episode_id=e.id {filter} ORDER BY e.id LIMIT 21 OFFSET {}",
            dataset.sql_name().replace('\'', "''"),
            slice.offset
        ))
    }
}

/// One discovered signal, aggregated by the agent to one value per episode.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SignalAnalysis {
    metric: DashboardMetric,
    sql: String,
}
impl SignalAnalysis {
    pub(super) fn new(metric: DashboardMetric, sql: String) -> Self {
        Self { metric, sql }
    }
    pub(super) fn metric(&self) -> &DashboardMetric {
        &self.metric
    }
}

#[derive(Clone, Debug)]
pub(super) struct ComparisonSlice {
    right: bool,
    offset: usize,
    bin: Option<(usize, f64, f64, bool)>,
}
impl ComparisonSlice {
    pub(super) fn new(right: bool) -> Self {
        Self {
            right,
            offset: 0,
            bin: None,
        }
    }
    pub(super) fn with_offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }
    pub(super) fn with_bin(mut self, bin: Option<(usize, f64, f64, bool)>) -> Self {
        self.bin = bin;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan() -> ComparisonPlan {
        serde_json::from_value(serde_json::json!({
            "title":"Compare","summary":"Comparable observations","dataset":null,"unit":"rows",
            "queries":{"left":"SELECT answer AS value FROM workflow_left","right":"SELECT answer AS value FROM workflow_right"},
            "metrics":[{"column":"value","label":"Answer","unit":"","show":true}]
        })).unwrap()
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn sources_remain_verbatim_and_metrics_share_ranges() {
        let plan = plan();
        let sql = plan
            .aggregate_sql(("SELECT 42 AS answer", Some("SELECT 43 AS answer")))
            .unwrap();
        assert!(sql.contains("workflow_left AS (SELECT 42 AS answer)"));
        assert!(sql.contains("workflow_right AS (SELECT 43 AS answer)"));
        assert!(sql.contains("MIN(value) AS low, MAX(value) AS high"));
        assert!(sql.contains("COUNT(CASE WHEN side=1 THEN 1 END)"));
        assert!(
            plan.aggregate_sql(("SELECT 1; DELETE FROM episodes", Some("SELECT 1")))
                .is_err()
        );
        assert!(plan.aggregate_sql(("SELECT 1", None)).is_err());
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn episode_counts_require_unique_identities_and_drilldown_uses_the_same_interpretation() {
        let mut plan = plan();
        plan.unit = ObservationUnit::Episodes;
        plan.dataset = Some(DatasetSelection::new(vec!["episodes".into()]).unwrap());
        plan.queries.left =
            "SELECT episode_id, duration_seconds AS value FROM workflow_left WHERE retained".into();
        plan.queries.right =
            "SELECT episode_id, duration_seconds AS value FROM workflow_right".into();
        let sources = (
            "SELECT id AS episode_id, true AS retained, 3 AS duration_seconds FROM episodes",
            None,
        );
        let sql = plan.aggregate_sql(sources).unwrap();
        assert!(sql.contains("COUNT(DISTINCT CASE WHEN side = 0 THEN episode_id END)"));
        assert!(sql.contains("workflow_right AS (SELECT * FROM episode_catalog)"));
        let evidence = plan
            .evidence_sql(
                sources,
                &ComparisonSlice::new(true).with_bin(Some((0, 1.0, 2.0, false))),
            )
            .unwrap();
        assert!(evidence.contains("FROM normalized_right n JOIN"));
        assert!(evidence.contains("< 2"));
        assert!(
            plan.evidence_sql(
                sources,
                &ComparisonSlice::new(false).with_bin(Some((0, f64::NAN, 2.0, false)))
            )
            .is_err()
        );
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn signal_enrichment_preserves_population_rules_and_missing_measurements() {
        let mut original = plan();
        original.unit = ObservationUnit::Episodes;
        original.dataset = Some(DatasetSelection::new(vec!["recordings".into()]).unwrap());
        original.queries.left =
            "SELECT episode_id, duration AS value FROM workflow_left WHERE retained".into();
        original.queries.right =
            "SELECT episode_id, duration_seconds AS value FROM workflow_right".into();
        let analysis: SignalAnalysis=serde_json::from_value(serde_json::json!({
            "metric":{"column":"temperature","label":"Motor temperature","unit":"°C","show":true,"description":"Maximum recorded temperature per episode","source":"stream.peak_temperature_celsius"},
            "sql":"SELECT id AS episode_id, MAX(temperature) AS value FROM sensor_rows GROUP BY id"
        })).unwrap();
        let mut secondary = analysis.clone();
        secondary.metric.show = false;
        assert!(!original.with_measurement(secondary).unwrap().metrics()[0].shown());
        let enriched = original.with_measurement(analysis).unwrap();
        let sql = enriched
            .aggregate_sql(("SELECT * FROM decisions", None))
            .unwrap();
        assert_eq!(sql.matches("LEFT JOIN comparison_measurements").count(), 2);
        assert!(sql.contains("FROM workflow_left WHERE retained"));
        assert!(sql.contains("comparison_measurements AS (SELECT id AS episode_id"));
        assert!(sql.contains("COUNT(DISTINCT CASE WHEN side = 0 THEN episode_id END)"));
        assert_eq!(original.metrics()[0].column(), "value");
        let evidence = enriched
            .evidence_sql(
                ("SELECT * FROM decisions", None),
                &ComparisonSlice::new(false),
            )
            .unwrap();
        assert!(evidence.contains("LEFT JOIN comparison_measurements"));
        let invalid: SignalAnalysis=serde_json::from_value(serde_json::json!({"metric":{"column":"bad","label":"Bad","unit":"","show":true,"description":"x","source":"x"},"sql":"DELETE FROM sensors"})).unwrap();
        assert!(original.with_measurement(invalid).is_err());
    }
}
