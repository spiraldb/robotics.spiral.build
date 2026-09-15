use super::{report::ComparisonReport, *};
use spiral_robotics_common::{relations::SpiralQuerySnapshot, workflow::document::WorkflowRun};

#[derive(Clone, PartialEq)]
pub(super) struct CompletedComparison {
    definition: WorkflowSpec,
    version: String,
    report: ComparisonReport,
    sql: String,
    elapsed_ms: u64,
}
impl CompletedComparison {
    pub(super) fn configuration(&self) -> &EpisodeComparison {
        self.definition
            .comparison()
            .expect("completed comparison has configuration")
    }
    pub(super) fn report(&self) -> &ComparisonReport {
        &self.report
    }
    pub(super) fn sql(&self) -> &str {
        &self.sql
    }
    pub(super) fn elapsed_ms(&self) -> u64 {
        self.elapsed_ms
    }
    pub(super) fn metric_index(&self, column: &str) -> Option<usize> {
        self.configuration()
            .metrics()
            .iter()
            .position(|metric| metric.column() == column)
    }
    pub(super) fn against(
        &self,
        other: &Self,
        metric: &ComparisonMetric,
    ) -> Result<MetricDistribution, String> {
        if self.configuration().dataset() != other.configuration().dataset()
            || self.version != other.version
        {
            return Err(
                "Comparing different datasets or dataset versions is not supported yet.".into(),
            );
        }
        if !matches!(metric.column(), "duration_seconds" | "message_count")
            && self.definition.sql() != other.definition.sql()
        {
            return Err("Comparing different measurement definitions is not supported yet.".into());
        }
        let left = self
            .metric_index(metric.column())
            .and_then(|index| self.report.metrics().get(index))
            .ok_or("Run this workflow to publish the metric.")?;
        let right = other
            .metric_index(metric.column())
            .and_then(|index| other.report.metrics().get(index))
            .ok_or("The baseline workflow does not publish this metric.")?;
        left.with_selected_baseline(right)
    }
    pub(super) fn from_run(run: &WorkflowRun) -> Result<Self, String> {
        let definition = run.workflow().clone();
        let config = definition
            .comparison()
            .ok_or("This workflow has no comparison output.")?;
        let version = run
            .snapshot()
            .comparison_version()
            .ok_or("This earlier run has no comparison output. Run the workflow to publish it.")?
            .to_owned();
        let snapshot = run
            .snapshot()
            .result()
            .ok_or("This run has no completed output.")?;
        let report = ComparisonReport::from_snapshot(snapshot, config.metrics().len())?;
        let sql = definition.execution_sql()?;
        Ok(Self {
            definition,
            version,
            report,
            sql,
            elapsed_ms: snapshot.elapsed_ms(),
        })
    }
    pub(super) async fn evidence(
        &self,
        selection: &ComparisonEvidence,
    ) -> Result<SpiralQuerySnapshot, String> {
        let config = self.configuration();
        if ComparisonExecution::version(config).await? != self.version {
            return Err(
                "The dataset changed. Run the comparison again before inspecting these episodes."
                    .into(),
            );
        }
        let sql = config.evidence_sql(self.definition.sql(), selection)?;
        let result = DatasetClient::current()
            .run_spiral_query(sql, 21)
            .await
            .map_err(|error| error.to_string())?;
        if ComparisonExecution::version(config).await? != self.version {
            return Err(
                "The dataset changed while loading evidence. Run the comparison again.".into(),
            );
        }
        Ok(result)
    }
}
struct ComparisonExecution;
impl ComparisonExecution {
    async fn version(config: &EpisodeComparison) -> Result<String, String> {
        let result = crate::activities::data::DatasetClient::current()
            .inspect_dataset(config.dataset().clone())
            .await
            .map_err(|e| e.to_string())?;
        Ok(result.dataset().version_id().into())
    }
}
