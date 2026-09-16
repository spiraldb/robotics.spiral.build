//! Incremental dashboard state: completed cards survive later analysis failures.
use super::agent::{ComparisonEvent, ComparisonResult};
use super::progress::{ComparisonPhase, ComparisonProgress};
use spiral_robotics_common::workflow::comparison_plan::DashboardMetric;

#[derive(Clone, Default, PartialEq)]
pub(super) struct ComparisonDashboard {
    population: Option<ComparisonResult>,
    cards: Vec<ComparisonCard>,
    pending: Vec<DashboardMetric>,
    failures: Vec<(DashboardMetric, String)>,
    progress: ComparisonProgress,
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
        self.progress.phase().map(ComparisonPhase::title)
    }
    pub(super) fn note(&self) -> &str {
        &self.note
    }
    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub(super) fn start(&mut self) {
        *self = Self {
            progress: ComparisonProgress::start(),
            ..Self::default()
        };
    }
    pub(super) fn finish(&mut self, result: Result<String, String>) {
        self.progress.finish();
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
            ComparisonEvent::Phase(phase) => self.progress.set_phase(phase),
            ComparisonEvent::Planned(metrics) => {
                self.progress.set_total(metrics.len());
                self.pending = metrics;
            }
            ComparisonEvent::PopulationReady(result) => {
                self.population = Some(result.clone());
                self.remove_pending(&result);
                self.append(result);
                self.progress.set_phase(ComparisonPhase::Measurements);
            }
            ComparisonEvent::MetricStarted(metric) => {
                self.progress.begin(metric.column());
                if !self
                    .pending
                    .iter()
                    .any(|pending| pending.column() == metric.column())
                {
                    self.pending.push(metric);
                }
            }
            ComparisonEvent::MetricReady(result) => {
                self.remove_pending(&result);
                self.append(result);
            }
            ComparisonEvent::MetricFailed((metric, error)) => {
                self.pending
                    .retain(|pending| pending.column() != metric.column());
                self.progress.end(metric.column());
                self.failures.push((metric, error));
            }
        }
    }
    pub(super) fn progress(&self) -> &ComparisonProgress {
        &self.progress
    }
    fn remove_pending(&mut self, result: &ComparisonResult) {
        for metric in result.plan().metrics() {
            self.pending
                .retain(|pending| pending.column() != metric.column());
            self.progress.end(metric.column());
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
