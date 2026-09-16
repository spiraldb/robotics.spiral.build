//! Factual progress from comparison events; no estimated percentages or model reasoning.
use super::dashboard::ComparisonDashboard;
use dioxus::prelude::*;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum ComparisonPhase {
    Workflows,
    Planning,
    Populations,
    Measurements,
}
impl ComparisonPhase {
    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Workflows => "Running workflows",
            Self::Planning => "Choosing measurements",
            Self::Populations => "Comparing populations",
            Self::Measurements => "Calculating measurements",
        }
    }
    fn detail(self) -> &'static str {
        match self {
            Self::Workflows => {
                "Running the saved queries and reading their results. Workflows that process video can take longer."
            }
            Self::Planning => {
                "Reviewing the workflow outputs and recorded signals to choose useful comparisons."
            }
            Self::Populations => {
                "Counting both groups and calculating their first shared measurement."
            }
            Self::Measurements => {
                "Cards appear as each calculation finishes. You can explore completed cards while the others run."
            }
        }
    }
}

#[derive(Clone, Default, PartialEq)]
pub(super) struct ComparisonProgress {
    phase: Option<ComparisonPhase>,
    started: Option<f64>,
    total: usize,
    running: BTreeSet<String>,
}
impl ComparisonProgress {
    pub(super) fn start() -> Self {
        Self {
            phase: Some(ComparisonPhase::Workflows),
            started: Some(js_sys::Date::now()),
            ..Self::default()
        }
    }
    pub(super) fn phase(&self) -> Option<ComparisonPhase> {
        self.phase
    }
    pub(super) fn set_phase(&mut self, phase: ComparisonPhase) {
        self.phase = Some(phase);
    }
    pub(super) fn set_total(&mut self, total: usize) {
        self.total = total;
    }
    pub(super) fn begin(&mut self, column: &str) {
        self.running.insert(column.into());
    }
    pub(super) fn end(&mut self, column: &str) {
        self.running.remove(column);
    }
    pub(super) fn running(&self, column: &str) -> bool {
        self.running.contains(column)
    }
    pub(super) fn finish(&mut self) {
        self.phase = None;
        self.running.clear();
    }
}

#[component]
pub(super) fn ComparisonProgressPanel(dashboard: ComparisonDashboard) -> Element {
    let progress = dashboard.progress();
    let Some(started) = progress.started else {
        return rsx! {};
    };
    let ready = dashboard.cards().len();
    let failed = dashboard.failures().len();
    let finished = ready + failed;
    let total = progress.total;
    let Some(phase) = progress.phase() else {
        return rsx! {};
    };
    let title = phase.title();
    rsx! {
        section {
            class: "comparison-progress",
            "aria-label": "Comparison progress",
            div { class: "comparison-progress-heading",
                h2 { role: "status", "{title}" }
                ComparisonElapsed { key: "{started}", started }
            }
                p { class: "comparison-progress-detail", "{phase.detail()}" }
                ol { class: "comparison-stages", "aria-label": "Analysis stages",
                    for (index, step) in [ComparisonPhase::Workflows, ComparisonPhase::Planning, ComparisonPhase::Populations, ComparisonPhase::Measurements].into_iter().enumerate() {
                        li {
                            class: if step < phase { "is-complete" } else if step == phase { "is-current" } else { "" },
                            "aria-current": if step == phase { "step" } else { "false" },
                            span { class: "comparison-stage-mark", "aria-hidden": "true",
                                if step < phase {
                                    svg { width: "12", height: "12", view_box: "0 0 16 16", fill: "none", stroke: "currentColor", stroke_width: "1.5", path { d: "M3 8l3 3 7-7" } }
                                } else { "{index + 1}" }
                            }
                            span { "{step.title()}" }
                        }
                    }
                }
            if total > 0 {
                div { class: "comparison-progress-counts", role: "status",
                    span { "{ready} of {total} measurements ready" }
                        span { "{progress.running.len()} running" }
                        if dashboard.pending().len() > progress.running.len() {
                            span { "{dashboard.pending().len() - progress.running.len()} queued" }
                        }
                    if failed > 0 { span { "{failed} unavailable" } }
                }
                progress { class: "comparison-measurement-progress", value: "{finished}", max: "{total}", "aria-label": "Measurements processed" }
            }
        }
    }
}

#[component]
fn ComparisonElapsed(started: f64) -> Element {
    let mut now = use_signal(js_sys::Date::now);
    use_future(move || async move {
        loop {
            gloo_timers::future::TimeoutFuture::new(1000).await;
            now.set(js_sys::Date::now());
        }
    });
    let seconds = ((now() - started).max(0.0) / 1000.0) as u64;
    let duration = if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    };
    rsx! { span { class: "comparison-elapsed", "aria-label": "Elapsed time", "{duration} elapsed" } }
}
