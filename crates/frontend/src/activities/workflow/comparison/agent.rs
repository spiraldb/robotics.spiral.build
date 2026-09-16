//! The comparison agent interprets ordinary outputs; its reasoning is never rendered.
use super::report::ComparisonReport;
use super::selection::{ComparisonEvidence, WorkflowSide};
use super::timing::ComparisonTiming;
use crate::activities::{data::DatasetClient, workflow::WorkflowClient};
use serde_json::{Value, json};
use spiral_robotics_common::{
    relations::SpiralQuerySnapshot,
    workflow::{
        comparison_inputs::ComparisonInputRequest,
        comparison_plan::{ComparisonPlan, ComparisonSlice, DashboardMetric},
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
    inputs: Option<super::inputs::ComparisonInputs>,
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
        let rows = self
            .inputs
            .as_ref()
            .ok_or("Comparison inputs expired. Compare again.")?
            .query(sql, 21)
            .await?;
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
    Phase(super::progress::ComparisonPhase),
    Planned(Vec<DashboardMetric>),
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
    inputs: super::inputs::ComparisonInputs,
    inspections: Vec<spiral_robotics_common::datasets::DatasetInspection>,
}
impl ComparisonSources {
    async fn load(ids: (String, Option<String>)) -> Result<Self, String> {
        let client = WorkflowClient::current();
        let data = DatasetClient::current();
        let (left, right, catalog) = ComparisonTiming::measure(
            "load documents and catalog",
            futures_util::future::try_join3(
                client.load(&ids.0),
                async {
                    match ids.1 {
                        Some(id) => client.load(&id).await.map(Some),
                        None => Ok(None),
                    }
                },
                data.list_datasets(),
            ),
        )
        .await
        .map_err(|error| error.to_string())?;
        let (inputs, inspections) = futures_util::future::try_join(
            ComparisonTiming::measure(
                "execute workflows once",
                super::inputs::ComparisonInputs::capture(ComparisonInputRequest::new(
                    left.workflow().clone(),
                    right.as_ref().map(|document| document.workflow().clone()),
                )),
            ),
            ComparisonTiming::measure("dataset inventories", async {
                futures_util::future::try_join_all(
                    catalog
                        .iter()
                        .map(|dataset| data.inspect_dataset(dataset.selection().clone())),
                )
                .await
                .map_err(|error| error.to_string())
            }),
        )
        .await?;
        Ok(Self {
            left,
            right,
            catalog,
            inputs,
            inspections,
        })
    }
    fn context(&self) -> Value {
        let output = self.inputs.output();
        json!({"left":Self::source(&self.left,output.left()),"right":self.right.as_ref().zip(output.right()).map(|(document,preview)|Self::source(document,preview)),"default_baseline":self.right.is_none(),"datasets":self.catalog,"dataset_inspections":self.inspections,"topic_sample_query":"SELECT value FROM spql.mcap.read_topic(\'<dataset from catalog>\', \'<topic from inspection>\', \'<variant from inspection>\') LIMIT 1"})
    }
    fn inspection(
        &self,
        dataset: &spiral_robotics_common::datasets::DatasetSelection,
    ) -> Result<&spiral_robotics_common::datasets::DatasetInspection, String> {
        self.inspections
            .iter()
            .find(|inspection| inspection.dataset().selection() == dataset)
            .ok_or_else(|| "Choose an inspected dataset.".into())
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
        let sources = self.inputs.sources();
        let sql = plan.aggregate_sql((&sources.0, sources.1.as_deref()))?;
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
        if let Some(dataset) = plan.dataset() {
            let initial = self
                .catalog
                .iter()
                .find(|candidate| candidate.selection() == dataset)
                .unwrap();
            if version.as_deref() != Some(initial.version_id()) {
                return Err(
                    "The dataset changed while reading workflow outputs. Compare again.".into(),
                );
            }
        }
        let output = self.inputs.query(sql, 100).await?;
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
            sources,
            inputs: Some(self.inputs.clone()),
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
                        sources.inspection(
                            base.plan()
                                .dataset()
                                .ok_or("Array measurements require an episode dataset.")?,
                        )?,
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
    fn decode(arguments: &str) -> Result<Self, String> {
        let batch: Self = serde_json::from_str(arguments).map_err(|error| error.to_string())?;
        batch.population.validate()?;
        let mut columns = std::collections::HashSet::new();
        for metric in batch
            .population
            .metrics()
            .iter()
            .chain(batch.cards.iter().map(PlannedSignal::metric))
        {
            if !columns.insert(metric.column()) {
                return Err("Choose a distinct column for each metric.".into());
            }
        }
        Ok(batch)
    }
    async fn execute(
        self,
        sources: &ComparisonSources,
        publish: &mut impl FnMut(ComparisonEvent),
    ) -> Result<String, String> {
        use futures_util::{StreamExt, stream::FuturesUnordered};
        publish(ComparisonEvent::Planned(
            self.population
                .metrics()
                .iter()
                .chain(self.cards.iter().map(PlannedSignal::metric))
                .cloned()
                .collect(),
        ));
        publish(ComparisonEvent::Phase(
            super::progress::ComparisonPhase::Populations,
        ));
        for metric in self.population.metrics() {
            publish(ComparisonEvent::MetricStarted(metric.clone()));
        }
        let base = ComparisonTiming::measure("population query", sources.evaluate(self.population))
            .await?;
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
                running.push(async move {
                    let result = ComparisonTiming::measure(
                        format!("card {}", metric.column()),
                        signal.evaluate(sources, base),
                    )
                    .await;
                    (metric, result)
                });
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
        publish(ComparisonEvent::Phase(
            super::progress::ComparisonPhase::Workflows,
        ));
        let sources = ComparisonSources::load(ids).await?;
        publish(ComparisonEvent::Phase(
            super::progress::ComparisonPhase::Planning,
        ));
        let context = sources.context();
        let mut messages = vec![
            ProviderMessage::text("system", Self::instructions()),
            ProviderMessage::text("user", context.to_string()),
        ];
        for round in 0..12 {
            let response = ComparisonTiming::measure(
                format!("model round {}", round + 1),
                WorkflowClient::current()
                    .complete(ModelRequest::new(messages.clone(), Self::tools()), |_| {}),
            )
            .await?;
            let calls = response.tool_calls().to_vec();
            messages.push(response);
            if calls.is_empty() {
                messages.push(ProviderMessage::text("user", "Use discovery tools as needed, then submit the entire dashboard in one plan_comparison call."));
                continue;
            }
            // Independent discovery calls from the same model turn also run together.
            let discoveries = futures_util::future::join_all(calls.iter().map(|call| async {
                if call.function().name() == "plan_comparison" {
                    return None;
                }
                Some(
                    ComparisonTiming::measure(
                        format!("discovery {}", call.function().name()),
                        super::discovery::SignalDiscovery::call(
                            call.function().name(),
                            call.function().arguments(),
                        ),
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
                            Err(error) => {
                                web_sys::console::log_1(
                                    &format!(
                                        "comparison_profile discovery failed tool={} error={error}",
                                        call.function().name()
                                    )
                                    .into(),
                                );
                                json!({"error":error})
                            }
                        }
                        .to_string(),
                    ));
                }
            }
            if let Some(call) = calls
                .iter()
                .find(|call| call.function().name() == "plan_comparison")
            {
                match ComparisonBatch::decode(call.function().arguments()) {
                    Ok(batch) => {
                        // Once a valid plan starts executing, native failures are terminal.
                        // Sending them back to the model hides the cause behind repeated turns.
                        return batch.execute(&sources, &mut publish).await;
                    }
                    Err(error) => {
                        web_sys::console::log_1(
                            &format!(
                                "comparison_profile invalid plan round={} error={error}",
                                round + 1
                            )
                            .into(),
                        );
                        messages.push(ProviderMessage::tool(
                            call.id().to_owned(),
                            json!({"error":error}).to_string(),
                        ));
                    }
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
            "description":{"type":"string","description":"Briefly define the per-episode measurement and aggregation. Include joint identity and units when relevant. Max 300 characters."},
            "source":{"type":"string","description":"Recorded topic and field name; max 200 characters."}
        },"required":["column","label","unit","show","description","source"],"additionalProperties":false})
    }
    fn tools() -> Vec<Value> {
        let mut tools = super::discovery::SignalDiscovery::tools();
        tools.push(json!({"type":"function","function":{"name":"plan_comparison","description":"Commit to a complete dashboard using the supplied context and any necessary samples. Choose useful measurements and submit promptly; exhaustive inspection is unnecessary. The application runs up to four cards concurrently and displays each result immediately.","parameters":{"type":"object","properties":{
            "population":{"type":"object","properties":{
                "title":{"type":"string"},"summary":{"type":"string"},"dataset":{"type":["object","null"],"properties":{"path":{"type":"array","items":{"type":"string"}}},"required":["path"],"additionalProperties":false},
                "unit":{"type":"string","enum":["episodes","rows"]},"queries":{"type":"object","description":"Normalization SQL over the already captured workflow_left/workflow_right CTEs. Each query must SELECT every column named in population.metrics, plus episode_id for episodes. Never redefine workflow_left/workflow_right or copy source workflow SQL; it has already executed.","properties":{"left":{"type":"string","description":"SELECT the chosen rows from workflow_left with every population metric column. Join episode_catalog for duration_seconds when it is absent from the captured output."},"right":{"type":"string","description":"SELECT the chosen rows from workflow_right with the same population metric columns. The default workflow_right already contains episode_id, duration_seconds and message_count."}},"required":["left","right"],"additionalProperties":false},
                "metrics":{"type":"array","minItems":1,"maxItems":8,"items":Self::metric_schema()}
            },"required":["title","summary","dataset","unit","queries","metrics"],"additionalProperties":false},
            "cards":{"type":"array","maxItems":12,"items":{"anyOf":[
                {"type":"object","properties":{"kind":{"type":"string","enum":["sql"]},"analysis":{"type":"object","properties":{"metric":Self::metric_schema(),"sql":{"type":"string","description":"Read-only SQL returning one native episode_id and numeric value per episode."}},"required":["metric","sql"],"additionalProperties":false}},"required":["kind","analysis"],"additionalProperties":false},
                {"type":"object","properties":{"kind":{"type":"string","enum":["array"]},"analysis":super::recorded_signal::RecordedArraySignal::definition()["function"]["parameters"]},"required":["kind","analysis"],"additionalProperties":false}
            ]}},
            "coverage_note":{"type":"string","description":"Empty string unless a short factual note materially changes interpretation of a chosen measurement. Focus on relevant ambiguity or unusual readings, not an inventory of missing sensors."}
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
            inputs: None,
        }
    }
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn planned_cards_distinguish_queued_running_and_failed_measurements() {
        let mut dashboard = ComparisonDashboard::default();
        dashboard.start();
        let base = population();
        let duration = base.plan().metrics()[0].clone();
        let temperature: DashboardMetric = serde_json::from_value(
            json!({"column":"temperature","label":"Temperature","unit":"°C","show":true}),
        )
        .unwrap();
        dashboard.accept(ComparisonEvent::Planned(vec![
            duration.clone(),
            temperature.clone(),
        ]));
        assert_eq!(dashboard.pending().len(), 2);
        assert!(!dashboard.progress().running("temperature"));
        dashboard.accept(ComparisonEvent::MetricStarted(duration));
        assert_eq!(dashboard.pending().len(), 2);
        dashboard.accept(ComparisonEvent::PopulationReady(base));
        assert_eq!(dashboard.cards().len(), 1);
        assert_eq!(dashboard.pending().len(), 1);
        assert!(!dashboard.progress().running("duration_seconds"));
        dashboard.accept(ComparisonEvent::MetricStarted(temperature.clone()));
        assert!(dashboard.progress().running("temperature"));
        dashboard.accept(ComparisonEvent::MetricFailed((
            temperature,
            "Unavailable".into(),
        )));
        assert!(dashboard.pending().is_empty());
        assert!(!dashboard.progress().running("temperature"));
        dashboard.finish(Ok(String::new()));
        assert_eq!(dashboard.cards().len(), 1);
        assert_eq!(dashboard.failures().len(), 1);
        dashboard.start();
        assert!(dashboard.cards().is_empty());
        assert!(dashboard.failures().is_empty());
        assert_eq!(
            dashboard.progress().phase(),
            Some(super::super::progress::ComparisonPhase::Workflows)
        );
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
