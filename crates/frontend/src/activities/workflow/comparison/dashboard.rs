//! Incremental dashboard state: completed cards survive later analysis failures.
use super::agent::{ComparisonEvent, ComparisonResult};
use spiral_robotics_common::workflow::comparison_plan::DashboardMetric;

#[derive(Clone, Default, PartialEq)]
pub(super) struct ComparisonDashboard {
    population: Option<ComparisonResult>,
    cards: Vec<ComparisonCard>,
    pending: Vec<DashboardMetric>,
    failures: Vec<(DashboardMetric, String)>,
    status: Option<String>,
    note: String,
    error: Option<String>,
}
#[derive(Clone, PartialEq)]
pub(super) struct ComparisonCard {
    result: ComparisonResult,
    index: usize,
    shown: bool,
}
impl ComparisonCard {
    pub(super) fn result(&self) -> &ComparisonResult {
        &self.result
    }
    pub(super) fn index(&self) -> usize {
        self.index
    }
    pub(super) fn metric(&self) -> &DashboardMetric {
        &self.result.plan().metrics()[self.index]
    }
    pub(super) fn shown(&self) -> bool {
        self.shown
    }
}
impl ComparisonDashboard {
    pub(super) fn population(&self) -> Option<&ComparisonResult> {
        self.population.as_ref()
    }
    pub(super) fn cards(&self) -> &[ComparisonCard] {
        &self.cards
    }
    pub(super) fn pending(&self) -> &[DashboardMetric] {
        &self.pending
    }
    pub(super) fn failures(&self) -> &[(DashboardMetric, String)] {
        &self.failures
    }
    pub(super) fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }
    pub(super) fn note(&self) -> &str {
        &self.note
    }
    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub(super) fn start(&mut self) {
        *self = Self {
            status: Some("Reading workflow outputs".into()),
            ..Self::default()
        };
    }
    pub(super) fn finish(&mut self, result: Result<String, String>) {
        self.status = None;
        self.pending.clear();
        match result {
            Ok(note) => self.note = note,
            Err(error) => self.error = Some(error),
        }
    }
    pub(super) fn set_shown(&mut self, index: usize, shown: bool) {
        if let Some(card) = self.cards.get_mut(index) {
            card.shown = shown;
        }
    }
    pub(super) fn accept(&mut self, event: ComparisonEvent) {
        match event {
            ComparisonEvent::Status(status) => self.status = Some(status.into()),
            ComparisonEvent::PopulationReady(result) => {
                self.population = Some(result.clone());
                self.append(result);
                self.status = Some("Inspecting recorded signals".into());
            }
            ComparisonEvent::MetricStarted(metric) => {
                self.status = Some("Analyzing recorded signals".into());
                self.pending.push(metric);
            }
            ComparisonEvent::MetricReady(result) => {
                self.pending.retain(|metric| {
                    !result
                        .plan()
                        .metrics()
                        .iter()
                        .any(|ready| ready.column() == metric.column())
                });
                self.append(result);
            }
            ComparisonEvent::MetricFailed((metric, error)) => {
                self.pending
                    .retain(|pending| pending.column() != metric.column());
                self.failures.push((metric, error));
            }
        }
    }
    fn append(&mut self, result: ComparisonResult) {
        for (index, metric) in result.plan().metrics().iter().enumerate() {
            self.cards.push(ComparisonCard {
                result: result.clone(),
                index,
                shown: metric.shown(),
            });
        }
    }
}
