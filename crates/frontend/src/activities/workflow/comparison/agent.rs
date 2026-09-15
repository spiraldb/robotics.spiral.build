//! The comparison agent interprets ordinary outputs; its reasoning is never rendered.
use super::report::ComparisonReport;
use super::selection::{ComparisonEvidence, WorkflowSide};
use crate::activities::{data::DatasetClient, workflow::WorkflowClient};
use serde_json::{Value, json};
use spiral_robotics_common::{
    relations::SpiralQuerySnapshot,
    workflow::{
        WorkflowSpec,
        comparison_plan::{ComparisonPlan, ComparisonSlice},
        conversation::ProviderMessage,
        document::{ModelRequest, WorkflowDocument},
    },
};

#[derive(Clone, PartialEq)]
pub(super) struct ComparisonResult {
    plan: ComparisonPlan,
    report: ComparisonReport,
    names: (String, String),
    sources: (String, Option<String>),
    version: Option<String>,
}
impl ComparisonResult {
    pub(super) fn plan(&self) -> &ComparisonPlan {
        &self.plan
    }
    pub(super) fn report(&self) -> &ComparisonReport {
        &self.report
    }
    pub(super) fn names(&self) -> &(String, String) {
        &self.names
    }
    pub(super) async fn evidence(
        &self,
        selection: &ComparisonEvidence,
    ) -> Result<SpiralQuerySnapshot, String> {
        let dataset = self
            .plan
            .dataset()
            .ok_or("This output has no episode dataset.")?;
        let data = DatasetClient::current();
        let before = data
            .inspect_dataset(dataset.clone())
            .await
            .map_err(|e| e.to_string())?;
        if self.version.as_deref() != Some(before.dataset().version_id()) {
            return Err("The dataset changed. Compare again before inspecting episodes.".into());
        }
        let bin = selection.bin().map(|(index, bucket)| {
            let (low, high) = self.report.metrics()[index].range();
            let width = (high - low) / 10.0;
            (
                index,
                low + width * bucket as f64,
                low + width * (bucket + 1) as f64,
                bucket == 9 || low == high,
            )
        });
        let slice = ComparisonSlice::new(selection.cohort() == WorkflowSide::Right)
            .with_offset(selection.offset())
            .with_bin(bin);
        let sql = self
            .plan
            .evidence_sql((&self.sources.0, self.sources.1.as_deref()), &slice)?;
        let rows = data
            .run_spiral_query(sql, 21)
            .await
            .map_err(|e| e.to_string())?;
        let after = data
            .inspect_dataset(dataset.clone())
            .await
            .map_err(|e| e.to_string())?;
        if self.version.as_deref() != Some(after.dataset().version_id()) {
            return Err("The dataset changed while loading episodes. Compare again.".into());
        }
        Ok(rows)
    }
}
pub(super) enum ComparisonEvent {
    Status(&'static str),
    PopulationReady(ComparisonResult),
    MetricStarted(spiral_robotics_common::workflow::comparison_plan::DashboardMetric),
    MetricReady(ComparisonResult),
    MetricFailed(
        (
            spiral_robotics_common::workflow::comparison_plan::DashboardMetric,
            String,
        ),
    ),
}
pub(super) struct ComparisonAgent;
struct ComparisonSources {
    left: WorkflowDocument,
    right: Option<WorkflowDocument>,
    catalog: Vec<spiral_robotics_common::datasets::DatasetSummary>,
}
impl ComparisonSources {
    async fn load(ids: (String, Option<String>)) -> Result<Self, String> {
        let client = WorkflowClient::current();
        let left = client
            .load(&ids.0)
            .await
            .map_err(|error| error.to_string())?;
        let right = match ids.1 {
            Some(id) => Some(client.load(&id).await.map_err(|error| error.to_string())?),
            None => None,
        };
        let catalog = DatasetClient::current()
            .list_datasets()
            .await
            .map_err(|error| error.to_string())?;
        Ok(Self {
            left,
            right,
            catalog,
        })
    }
    async fn context(&self) -> Result<Value, String> {
        let data = DatasetClient::current();
        let left = data
            .run_spiral_query(self.left.workflow().sql().into(), 8)
            .await
            .map_err(|error| error.to_string())?;
        let right = match &self.right {
            Some(document) => Some(
                data.run_spiral_query(document.workflow().sql().into(), 8)
                    .await
                    .map_err(|error| error.to_string())?,
            ),
            None => None,
        };
        Ok(
            json!({"left":Self::source(&self.left,&left),"right":self.right.as_ref().zip(right.as_ref()).map(|(document,preview)|Self::source(document,preview)),"default_baseline":self.right.is_none(),"datasets":self.catalog}),
        )
    }
    async fn evaluate(&self, plan: ComparisonPlan) -> Result<ComparisonResult, String> {
        let data = DatasetClient::current();
        if let Some(dataset) = plan.dataset()
            && !self
                .catalog
                .iter()
                .any(|candidate| candidate.selection() == dataset)
        {
            return Err("Choose a dataset from the supplied catalog.".into());
        }
        let sql = plan.aggregate_sql((
            self.left.workflow().sql(),
            self.right
                .as_ref()
                .map(|document| document.workflow().sql()),
        ))?;
        let definition = WorkflowSpec::new("comparison-analysis", "Comparison").with_sql(&sql);
        let planned = WorkflowClient::current()
            .plan(&definition)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(error) = planned.error() {
            return Err(error.into());
        }
        let version = match plan.dataset() {
            Some(dataset) => Some(
                data.inspect_dataset(dataset.clone())
                    .await
                    .map_err(|error| error.to_string())?
                    .dataset()
                    .version_id()
                    .to_owned(),
            ),
            None => None,
        };
        let output = data
            .run_spiral_query(sql, 100)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(dataset) = plan.dataset() {
            let after = data
                .inspect_dataset(dataset.clone())
                .await
                .map_err(|error| error.to_string())?;
            if version.as_deref() != Some(after.dataset().version_id()) {
                return Err("The dataset changed during analysis. Compare again.".into());
            }
        }
        let report = ComparisonReport::from_snapshot(&output, plan.metrics().len())?;
        Ok(ComparisonResult {
            plan,
            report,
            version,
            sources: (
                self.left.workflow().sql().into(),
                self.right
                    .as_ref()
                    .map(|document| document.workflow().sql().into()),
            ),
            names: (
                self.left.workflow().name().into(),
                self.right
                    .as_ref()
                    .map(|document| document.workflow().name().into())
                    .unwrap_or_else(|| "All episodes".into()),
            ),
        })
    }
    fn source(document: &WorkflowDocument, preview: &SpiralQuerySnapshot) -> Value {
        let rows = preview
            .rows()
            .iter()
            .map(|row| {
                row.iter()
                    .filter_map(|(name, value)| {
                        let safe = match value {
                            Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
                            Value::String(s) if !s.starts_with("data:") && s.len() < 400 => {
                                value.clone()
                            }
                            _ => return None,
                        };
                        Some((name.clone(), safe))
                    })
                    .collect::<serde_json::Map<_, _>>()
            })
            .collect::<Vec<_>>();
        let context = document
            .conversation()
            .session()
            .messages()
            .iter()
            .rev()
            .filter(|m| !m.text().is_empty())
            .take(2)
            .map(|m| m.text().chars().take(2000).collect::<String>())
            .collect::<Vec<_>>();
        json!({"name":document.workflow().name(),"sql":document.workflow().sql(),"columns":preview.columns(),"sample":rows,"sample_is_truncated":preview.truncated(),"context":context})
    }
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ComparisonBatch {
    population: ComparisonPlan,
    cards: Vec<PlannedSignal>,
    coverage_note: String,
}
#[derive(serde::Deserialize)]
#[serde(tag = "kind", content = "analysis", rename_all = "snake_case")]
enum PlannedSignal {
    Sql(spiral_robotics_common::workflow::comparison_plan::SignalAnalysis),
    Array(super::recorded_signal::RecordedArraySignal),
}
impl PlannedSignal {
    fn metric(&self) -> &spiral_robotics_common::workflow::comparison_plan::DashboardMetric {
        match self {
            Self::Sql(signal) => signal.metric(),
            Self::Array(signal) => signal.metric(),
        }
    }
    async fn evaluate(
        self,
        sources: &ComparisonSources,
        base: &ComparisonResult,
    ) -> Result<ComparisonResult, String> {
        let analysis = match self {
            Self::Sql(signal) => signal,
            Self::Array(signal) => {
                signal
                    .prepare(
                        base.plan()
                            .dataset()
                            .ok_or("Array measurements require an episode dataset.")?,
                    )
                    .await?
            }
        };
        let result = sources
            .evaluate(base.plan().with_measurement(analysis)?)
            .await?;
        if result.version != base.version || result.report().counts() != base.report().counts() {
            return Err("The episode populations or dataset changed. Compare again.".into());
        }
        if result.report().metrics()[0].counts() == (0, 0) {
            return Err("No recorded values were found in either population.".into());
        }
        Ok(result)
    }
}
impl ComparisonBatch {
    async fn execute(
        self,
        sources: &ComparisonSources,
        publish: &mut impl FnMut(ComparisonEvent),
    ) -> Result<String, String> {
        use futures_util::{StreamExt, stream::FuturesUnordered};
        let mut columns = std::collections::HashSet::new();
        for metric in self
            .population
            .metrics()
            .iter()
            .chain(self.cards.iter().map(PlannedSignal::metric))
        {
            if !columns.insert(metric.column()) {
                return Err("Choose a distinct column for each metric.".into());
            }
        }
        publish(ComparisonEvent::Status("Comparing episode populations"));
        let base = sources.evaluate(self.population).await?;
        if base.report().counts().0 > 0
            && base.report().counts().1 > 0
            && base.report().metrics().iter().any(|metric| {
                let measured = metric.counts();
                measured.0 == 0 || measured.1 == 0
            })
        {
            return Err("An initial metric has no measured values on one side. Establish populations using shared duration, and derive other signals for both sides in cards.".into());
        }
        publish(ComparisonEvent::PopulationReady(base.clone()));
        let mut queued = self.cards.into_iter();
        let mut running = FuturesUnordered::new();
        loop {
            while running.len() < 4 {
                let Some(signal) = queued.next() else { break };
                let metric = signal.metric().clone();
                publish(ComparisonEvent::MetricStarted(metric.clone()));
                let base = &base;
                running.push(async move { (metric, signal.evaluate(sources, base).await) });
            }
            let Some((metric, result)) = running.next().await else {
                break;
            };
            match result {
                Ok(result) => publish(ComparisonEvent::MetricReady(result)),
                Err(error) => publish(ComparisonEvent::MetricFailed((metric, error))),
            }
        }
        Ok(self.coverage_note.chars().take(600).collect())
    }
}

impl ComparisonAgent {
    pub(super) async fn compare(
        ids: (String, Option<String>),
        mut publish: impl FnMut(ComparisonEvent),
    ) -> Result<String, String> {
        publish(ComparisonEvent::Status("Reading workflow outputs"));
        let sources = ComparisonSources::load(ids).await?;
        let context = sources.context().await?;
        let mut messages = vec![
            ProviderMessage::text("system", Self::instructions()),
            ProviderMessage::text("user", context.to_string()),
        ];
        for _ in 0..12 {
            let response = WorkflowClient::current()
                .complete(ModelRequest::new(messages.clone(), Self::tools()), |_| {})
                .await?;
            let calls = response.tool_calls().to_vec();
            messages.push(response);
            if calls.is_empty() {
                messages.push(ProviderMessage::text("user", "Use discovery tools as needed, then submit the entire dashboard in one plan_comparison call."));
                continue;
            }
            // Independent discovery calls from the same model turn also run together.
            publish(ComparisonEvent::Status("Inspecting recorded signals"));
            let discoveries = futures_util::future::join_all(calls.iter().map(|call| async {
                if call.function().name() == "plan_comparison" {
                    return None;
                }
                Some(
                    super::discovery::SignalDiscovery::call(
                        call.function().name(),
                        call.function().arguments(),
                    )
                    .await,
                )
            }))
            .await;
            for (call, discovery) in calls.iter().zip(discoveries) {
                if let Some(attempt) = discovery {
                    messages.push(ProviderMessage::tool(
                        call.id().to_owned(),
                        match attempt {
                            Ok(value) => value,
                            Err(error) => json!({"error":error}),
                        }
                        .to_string(),
                    ));
                }
            }
            if let Some(call) = calls
                .iter()
                .find(|call| call.function().name() == "plan_comparison")
            {
                let attempt = async {
                    let batch: ComparisonBatch = serde_json::from_str(call.function().arguments())
                        .map_err(|error| error.to_string())?;
                    batch.execute(&sources, &mut publish).await
                }
                .await;
                // Invalid plans can be repaired before execution. Once cards are running,
                // failures are reported together without another model round trip.
                match attempt {
                    Ok(note) => return Ok(note),
                    Err(error) => messages.push(ProviderMessage::tool(
                        call.id().to_owned(),
                        json!({"error":error}).to_string(),
                    )),
                }
            }
        }
        Err("Could not prepare the comparison. Compare again to retry.".into())
    }
    fn instructions() -> &'static str {
        include_str!("instructions.txt")
    }
    pub(super) fn metric_schema() -> Value {
        json!({"type":"object","properties":{
            "column":{"type":"string"},"label":{"type":"string"},"unit":{"type":"string"},"show":{"type":"boolean"},
            "description":{"type":"string","description":"Explain exactly what one episode value means: aggregation, joint identity, validity and units. Max 300 characters."},
            "source":{"type":"string","description":"Recorded topic and field name; max 200 characters."}
        },"required":["column","label","unit","show","description","source"],"additionalProperties":false})
    }
    fn tools() -> Vec<Value> {
        let mut tools = super::discovery::SignalDiscovery::tools();
        tools.push(json!({"type":"function","function":{"name":"plan_comparison","description":"Submit the complete dashboard once, after inspecting schemas and representative values. The application establishes the populations, runs up to four cards concurrently, and displays each result immediately. No further model turn is needed.","parameters":{"type":"object","properties":{
            "population":{"type":"object","properties":{
                "title":{"type":"string"},"summary":{"type":"string"},"dataset":{"type":["object","null"],"properties":{"path":{"type":"array","items":{"type":"string"}}},"required":["path"],"additionalProperties":false},
                "unit":{"type":"string","enum":["episodes","rows"]},"queries":{"type":"object","properties":{"left":{"type":"string"},"right":{"type":"string"}},"required":["left","right"],"additionalProperties":false},
                "metrics":{"type":"array","minItems":1,"maxItems":8,"items":Self::metric_schema()}
            },"required":["title","summary","dataset","unit","queries","metrics"],"additionalProperties":false},
            "cards":{"type":"array","maxItems":12,"items":{"anyOf":[
                {"type":"object","properties":{"kind":{"type":"string","enum":["sql"]},"analysis":{"type":"object","properties":{"metric":Self::metric_schema(),"sql":{"type":"string","description":"Read-only SQL returning one native episode_id and numeric value per episode."}},"required":["metric","sql"],"additionalProperties":false}},"required":["kind","analysis"],"additionalProperties":false},
                {"type":"object","properties":{"kind":{"type":"string","enum":["array"]},"analysis":super::recorded_signal::RecordedArraySignal::definition()["function"]["parameters"]},"required":["kind","analysis"],"additionalProperties":false}
            ]}},
            "coverage_note":{"type":"string","description":"Brief factual note about unavailable sensors, uncertain units or suspicious recorded readings. Do not predict computed results."}
        },"required":["population","cards","coverage_note"],"additionalProperties":false}}}));
        tools
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod tests {
    use super::super::dashboard::ComparisonDashboard;
    use super::*;

    fn population() -> ComparisonResult {
        let plan=serde_json::from_value(json!({"title":"Episodes","summary":"Retained vs all","dataset":{"path":["recordings"]},"unit":"episodes","queries":{"left":"SELECT episode_id, duration_seconds FROM workflow_left WHERE retained","right":"SELECT episode_id, duration_seconds FROM workflow_right"},"metrics":[{"column":"duration_seconds","label":"Duration","unit":"s","show":true}]})).unwrap();
        let rows=vec![json!({"metric":-1,"bucket":-1,"low":null,"high":null,"left_n":2,"right_n":3,"left_sum":null,"right_sum":null,"invalid_count":0})].into_iter().map(|value|value.as_object().unwrap().clone()).collect();
        let report =
            ComparisonReport::from_snapshot(&SpiralQuerySnapshot::new(vec![], rows, false), 1)
                .unwrap();
        ComparisonResult {
            plan,
            report,
            names: ("A".into(), "B".into()),
            sources: ("SELECT 1".into(), None),
            version: Some("v1".into()),
        }
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn completed_cards_are_visible_during_analysis_and_survive_a_later_failure() {
        let mut dashboard = ComparisonDashboard::default();
        dashboard.start();
        let result = population();
        dashboard.accept(ComparisonEvent::PopulationReady(result.clone()));
        assert_eq!(dashboard.cards().len(), 1);
        assert!(dashboard.status().is_some());
        let metric=serde_json::from_value(json!({"column":"temperature","label":"Motor temperature","unit":"°C","show":true,"description":"Maximum recorded temperature per episode","source":"stream.peak_temperature_celsius"})).unwrap();
        dashboard.accept(ComparisonEvent::MetricStarted(metric));
        assert!(!dashboard.pending().is_empty());
        assert_eq!(dashboard.population().unwrap().report().counts(), (2, 3));
        dashboard.set_shown(0, false);
        dashboard.finish(Err("Connection interrupted".into()));
        assert_eq!(dashboard.cards().len(), 1);
        assert!(!dashboard.cards()[0].shown());
        assert!(dashboard.status().is_none());
        assert!(dashboard.pending().is_empty());
        assert!(dashboard.error().is_some());
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn parallel_completions_only_remove_their_own_pending_card() {
        use spiral_robotics_common::workflow::comparison_plan::{DashboardMetric, SignalAnalysis};
        let mut dashboard = ComparisonDashboard::default();
        dashboard.start();
        let base = population();
        dashboard.accept(ComparisonEvent::PopulationReady(base.clone()));
        let temperature: DashboardMetric = serde_json::from_value(json!({"column":"temperature","label":"Temperature","unit":"°C","show":true,"description":"Recorded peak per episode","source":"stream.peak_temperature"})).unwrap();
        let voltage: DashboardMetric = serde_json::from_value(json!({"column":"voltage","label":"Voltage","unit":"V","show":true,"description":"Recorded minimum per episode","source":"stream.min_voltage"})).unwrap();
        dashboard.accept(ComparisonEvent::MetricStarted(temperature.clone()));
        dashboard.accept(ComparisonEvent::MetricStarted(voltage.clone()));
        dashboard.set_shown(0, false);
        let mut completed = base.clone();
        completed.plan = base
            .plan
            .with_measurement(SignalAnalysis::new(
                voltage,
                "SELECT episode_id, 12 AS value FROM episode_catalog".into(),
            ))
            .unwrap();
        dashboard.accept(ComparisonEvent::MetricReady(completed));
        assert_eq!(dashboard.pending().len(), 1);
        assert_eq!(dashboard.pending()[0].column(), "temperature");
        assert_eq!(dashboard.cards()[1].metric().column(), "voltage");
        assert!(!dashboard.cards()[0].shown());
        dashboard.accept(ComparisonEvent::MetricFailed((
            temperature,
            "Missing readings".into(),
        )));
        dashboard.finish(Ok("Recorded sensors only".into()));
        assert!(dashboard.pending().is_empty());
        assert_eq!(dashboard.failures().len(), 1);
        assert_eq!(dashboard.cards().len(), 2);
        assert_eq!(dashboard.population().unwrap().report().counts(), (2, 3));
    }
}
