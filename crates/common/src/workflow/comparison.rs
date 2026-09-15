//! A saved episode comparison. Spiral computes populations and histogram bins;
//! the browser only presents the aggregate rows and requests bounded evidence.
use crate::datasets::DatasetSelection;
use crate::sql::{ReadOnlyQuery, ReadOnlyQueryMode};
use serde::{Deserialize, Serialize};

mod query;
pub use query::{ComparisonEvidence, EvidenceCohort};

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EpisodeComparison {
    dataset: DatasetSelection,
    description: String,
    criterion: ComparisonCriterion,
    metrics: Vec<ComparisonMetric>,
    #[serde(default)]
    baseline: ComparisonBaseline,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    baseline_workflow: Option<String>,
}
impl EpisodeComparison {
    pub fn new(dataset: DatasetSelection) -> Self {
        Self {
            dataset,
            description: "Episodes longer than 30 seconds".into(),
            criterion: ComparisonCriterion::new(
                "duration_seconds",
                ComparisonOperator::Greater,
                "30",
            ),
            metrics: vec![ComparisonMetric::duration(), ComparisonMetric::messages()],
            baseline: ComparisonBaseline::All,
            baseline_workflow: None,
        }
    }
    pub fn dataset(&self) -> &DatasetSelection {
        &self.dataset
    }
    pub fn description(&self) -> &str {
        &self.description
    }
    pub fn set_description(&mut self, description: String) {
        self.description = description;
    }
    pub fn criterion(&self) -> &ComparisonCriterion {
        &self.criterion
    }
    pub fn set_criterion(&mut self, criterion: ComparisonCriterion) {
        self.criterion = criterion;
    }
    pub fn metrics(&self) -> &[ComparisonMetric] {
        &self.metrics
    }
    pub fn set_metrics(&mut self, metrics: Vec<ComparisonMetric>) {
        self.metrics = metrics;
    }
    pub fn baseline(&self) -> ComparisonBaseline {
        self.baseline
    }
    pub fn set_baseline(&mut self, baseline: ComparisonBaseline) {
        self.baseline = baseline;
    }
    pub fn baseline_workflow(&self) -> Option<&str> {
        self.baseline_workflow.as_deref()
    }
    pub fn set_baseline_workflow(&mut self, workflow: Option<String>) {
        self.baseline_workflow = workflow;
        self.baseline = ComparisonBaseline::All;
    }
    pub fn validate(&self) -> Result<(), String> {
        DatasetSelection::new(self.dataset.path().to_vec())?;
        if self.description.len() > 2000 {
            return Err("Keep the criterion explanation under 2,000 characters.".into());
        }
        self.criterion.validate()?;
        if self.metrics.is_empty() || self.metrics.len() > 6 {
            return Err("Choose between one and six comparison metrics.".into());
        }
        let mut columns = std::collections::BTreeSet::new();
        for metric in &self.metrics {
            metric.validate()?;
            if !columns.insert(metric.column()) {
                return Err("Each comparison metric must be unique.".into());
            }
        }
        Ok(())
    }
    pub fn population_sql(&self) -> String {
        format!(
            "SELECT id AS episode_id FROM spql.mcap.episodes('{}')",
            self.dataset.sql_name().replace('\'', "''")
        )
    }
    fn validate_population(&self, sql: &str) -> Result<(), String> {
        self.validate()?;
        ReadOnlyQuery::new(sql, ReadOnlyQueryMode::Workflow)
            .authorize()
            .map_err(|e| e.to_string())
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonBaseline {
    #[default]
    All,
    Remaining,
}
impl ComparisonBaseline {
    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All episodes",
            Self::Remaining => "Remaining episodes",
        }
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonCriterion {
    column: String,
    operator: ComparisonOperator,
    threshold: String,
}
impl ComparisonCriterion {
    pub fn new(
        column: impl Into<String>,
        operator: ComparisonOperator,
        threshold: impl Into<String>,
    ) -> Self {
        Self {
            column: column.into(),
            operator,
            threshold: threshold.into(),
        }
    }
    pub fn column(&self) -> &str {
        &self.column
    }
    pub fn operator(&self) -> ComparisonOperator {
        self.operator
    }
    pub fn threshold(&self) -> &str {
        &self.threshold
    }
    fn validate(&self) -> Result<(), String> {
        ComparisonMetric::new(&self.column, &self.column, "").validate()?;
        self.threshold
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .ok_or("Enter a finite numeric threshold.")?;
        Ok(())
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ComparisonOperator {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}
impl ComparisonOperator {
    pub fn sql(self) -> &'static str {
        match self {
            Self::Less => "<",
            Self::LessEqual => "<=",
            Self::Greater => ">",
            Self::GreaterEqual => ">=",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Less => "is below",
            Self::LessEqual => "is at most",
            Self::Greater => "is above",
            Self::GreaterEqual => "is at least",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Less => "less",
            Self::LessEqual => "less_equal",
            Self::Greater => "greater",
            Self::GreaterEqual => "greater_equal",
        }
    }
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "less" => Some(Self::Less),
            "less_equal" => Some(Self::LessEqual),
            "greater" => Some(Self::Greater),
            "greater_equal" => Some(Self::GreaterEqual),
            _ => None,
        }
    }
}

#[cfg_attr(feature = "http-schema", derive(schemars::JsonSchema))]
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ComparisonMetric {
    column: String,
    label: String,
    unit: String,
}
impl ComparisonMetric {
    pub fn new(
        column: impl Into<String>,
        label: impl Into<String>,
        unit: impl Into<String>,
    ) -> Self {
        Self {
            column: column.into(),
            label: label.into(),
            unit: unit.into(),
        }
    }
    pub fn duration() -> Self {
        Self::new("duration_seconds", "Episode duration", "s")
    }
    pub fn messages() -> Self {
        Self::new("message_count", "Recorded messages", "")
    }
    pub fn column(&self) -> &str {
        &self.column
    }
    pub fn label(&self) -> &str {
        &self.label
    }
    pub fn unit(&self) -> &str {
        &self.unit
    }
    fn validate(&self) -> Result<(), String> {
        if self.column.is_empty()
            || self.column.len() > 128
            || self.column.chars().any(char::is_control)
        {
            return Err("Metric columns must contain 1–128 printable characters.".into());
        }
        if self.label.is_empty() || self.label.len() > 128 || self.unit.len() > 32 {
            return Err("Provide a short metric label and unit.".into());
        }
        Ok(())
    }
    fn expression(&self) -> String {
        Self::column_expression(&self.column)
    }
    fn column_expression(column: &str) -> String {
        match column {
            "duration_seconds" => "CAST(e.duration_ns AS DOUBLE) / 1000000000.0".into(),
            "message_count" => "CAST(e.message_count AS DOUBLE)".into(),
            _ => format!("CAST(m.\"{}\" AS DOUBLE)", column.replace('"', "\"\"")),
        }
    }
}

#[cfg(test)]
mod tests;
