//! Progressive comparisons of ordinary workflow outputs and recorded signals.
mod agent;
mod dashboard;
mod discovery;
mod evidence;
mod inputs;
mod progress;
mod recorded_signal;
mod report;
mod selection;
mod timing;

use super::state::WorkflowController;
use crate::components::icons::{SpinnerIcon, WorkflowIcon};
use crate::components::interaction::{Action, ActionPurpose, ActionVariant, BrowserFocus};
use agent::{ComparisonAgent, ComparisonResult};
use dashboard::{ComparisonCard, ComparisonDashboard};
use dioxus::prelude::*;
use report::MetricNumber;
use selection::{ComparisonEvidence, WorkflowSide};
use spiral_robotics_common::workflow::comparison_plan::{DashboardMetric, ObservationUnit};

#[derive(Clone, PartialEq)]
struct EvidenceRequest {
    result: ComparisonResult,
    selection: ComparisonEvidence,
}

/// Compare shares the Build bar: the view switch, then the pair of workflows
/// and the Compare action, so switching views never moves the header.
#[component]
pub(super) fn WorkflowComparison(
    controller: WorkflowController,
    comparing: Signal<bool>,
) -> Element {
    // Workflow A follows the workflow open in Build, including one saved a
    // moment ago, until the engineer picks another one here.
    let mut chosen_left = use_signal(|| None::<String>);
    let left = use_memo(move || chosen_left().unwrap_or_else(|| controller.spec().id().to_owned()));
    let mut right = use_signal(String::new);
    let mut dashboard = use_signal(ComparisonDashboard::default);
    let mut evidence_request = use_signal(|| None::<EvidenceRequest>);
    let mut inspected = use_signal(|| None::<serde_json::Map<String, serde_json::Value>>);
    let workflows = controller.workflows();
    let valid_left = workflows.iter().any(|workflow| workflow.id() == left());
    let current = dashboard();
    let busy = current.status().is_some();
    let inspect = move |request: EvidenceRequest| {
        evidence_request.set(Some(request));
        BrowserFocus::after_render("comparison-evidence-heading".into());
    };
    rsx! {
        section { class: "episode-comparison comparison-workspace", "aria-label": "Workflow comparison",
            header { class: "document-bar comparison-pair-bar",
                span { class: "document-glyph", "aria-hidden": "true", WorkflowIcon {} }
                super::workspace::WorkflowViewSwitch { comparing }
                div { class: "comparison-pair", role: "group", "aria-label": "Workflows to compare",
                    label { class: "comparison-side",
                        span { class: "comparison-side-key", "A" }
                        select {
                            class: "comparison-select",
                            "aria-label": "Workflow A", value: left(), disabled: busy,
                            onchange: move |event| {
                                chosen_left.set(Some(event.value()));
                                dashboard.set(ComparisonDashboard::default());
                                evidence_request.set(None);
                                inspected.set(None);
                            },
                            if !valid_left { option { value: "", "Choose a saved workflow" } }
                            for workflow in &workflows {
                                option { value: workflow.id(), selected: workflow.id()==left(), "{workflow.name()}" }
                            }
                        }
                    }
                    span { class: "comparison-pair-against", "against" }
                    label { class: "comparison-side",
                        span { class: "comparison-side-key", "B" }
                        select {
                            class: "comparison-select",
                            "aria-label": "Workflow B", value: right(), disabled: busy,
                            onchange: move |event| {
                                right.set(event.value());
                                dashboard.set(ComparisonDashboard::default());
                                evidence_request.set(None);
                                inspected.set(None);
                            },
                            option { value: "", selected: right().is_empty(), "All episodes" }
                            for workflow in &workflows {
                                option { value: workflow.id(), selected: workflow.id()==right(), "{workflow.name()}" }
                            }
                        }
                    }
                }
                div { class: "document-actions",
                    Action {
                        purpose: ActionPurpose::Start, variant: ActionVariant::Solid,
                        class: "comparison-primary document-run-action",
                        disabled: busy || !valid_left, aria_busy: busy,
                        title: if !valid_left { "Choose a saved workflow for A" } else { "Run both workflows and measure their episodes" },
                        onclick: move |_| {
                            let selected_right=right();
                            let ids=(left(),(!selected_right.is_empty()).then_some(selected_right));
                            dashboard.write().start();
                            evidence_request.set(None);
                            inspected.set(None);
                            spawn(async move {
                                let outcome=timing::ComparisonTiming::measure("total", ComparisonAgent::compare(ids,move |event| dashboard.write().accept(event))).await;
                                dashboard.write().finish(outcome);
                            });
                        },
                        if busy { SpinnerIcon {} "Analyzing" } else { "Compare" }
                    }
                }
            }
            div { class: "comparison-content",
                if let Some(population)=current.population() {
                    div { class: "comparison-results-toolbar",
                        ComparisonCounts { result: population.clone(), on_inspect: inspect }
                        details { class: "comparison-metrics-menu",
                            summary { "Metrics ({current.cards().len()})" }
                            div { class: "comparison-metric-options",
                                for (index,card) in current.cards().iter().enumerate() {
                                    label {
                                        input {
                                            r#type: "checkbox", checked: card.shown(),
                                            onchange: move |event| dashboard.write().set_shown(index,event.checked()),
                                        }
                                        "{card.metric().label()}"
                                    }
                                }
                            }
                        }
                    }
                }
                if busy { progress::ComparisonProgressPanel { dashboard: current.clone() } }
                if let Some(message)=current.error() {
                    div { class: "comparison-error", role: "alert",
                        strong { "Analysis stopped" }
                        p { "{message}" }
                    }
                }
                if let Some(row)=inspected() {
                    if let Some(dataset)=evidence_request().and_then(|request|request.result.plan().dataset().cloned()) {
                        div { class: "comparison-inspector",
                            crate::activities::data::ComparisonEpisodeInspector { dataset, row, on_back: move |_|inspected.set(None) }
                        }
                    }
                }
                div { class: if inspected().is_some() { "workflow-mode-hidden" } else { "comparison-review" },
                    if current.population().is_some() || !current.pending().is_empty() {
                        div { class: "comparison-card-grid",
                            for card in current.cards().iter().filter(|card|card.shown()) {
                                MetricCard { card: card.clone(), on_inspect: inspect }
                            }
                            for metric in current.pending() { PendingMetricCard { key: "{metric.column()}", metric: metric.clone(), running: current.progress().running(metric.column()) } }
                            if !current.failures().is_empty() {
                                UnavailableMeasurements { failures: current.failures().to_vec() }
                            }
                        }
                        if !current.cards().iter().any(|card|card.shown()) && current.pending().is_empty() && current.failures().is_empty() {
                            p { class: "comparison-empty", "Choose a metric from the Metrics menu to show its card." }
                        }
                        if !current.note().is_empty() { p { class: "comparison-coverage-note", "{current.note()}" } }
                        if let Some(request)=evidence_request() {
                            evidence::ComparisonEpisodes {
                                result: request.result.clone(), selection: request.selection.clone(),
                                on_select: move |selection| evidence_request.set(Some(EvidenceRequest{result:request.result.clone(),selection})),
                                on_inspect: move |row|inspected.set(Some(row)),
                            }
                        }
                    } else if !busy && current.error().is_none() {
                        div { class: "comparison-empty",
                            h2 { "What changes between these workflows?" }
                            p { "Compare their episodes, motion, and recorded sensor signals. Each measurement appears as soon as its analysis finishes." }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn ComparisonCounts(
    result: ComparisonResult,
    on_inspect: EventHandler<EvidenceRequest>,
) -> Element {
    let (left, right) = result.report().counts();
    let unit = result.plan().unit().label();
    let inspectable =
        result.plan().unit() == ObservationUnit::Episodes && result.plan().dataset().is_some();
    rsx! {
        div { class: "comparison-population-summary", "aria-label": "Compared populations",
            for (side,count,label) in [(WorkflowSide::Left,left,"A"),(WorkflowSide::Right,right,"B")] {
                button {
                    class: if side==WorkflowSide::Left { "comparison-population-link is-selected" } else { "comparison-population-link" },
                    disabled: !inspectable, title: format!("Inspect {}", if side==WorkflowSide::Left { &result.names().0 } else { &result.names().1 }),
                    onclick: { let result=result.clone(); move |_|on_inspect.call(EvidenceRequest{result:result.clone(),selection:ComparisonEvidence::new(side)}) },
                    span { class: "comparison-side-label", "{label}" }
                    strong { "{count}" }
                    span { "{unit}" }
                }
            }
        }
    }
}

#[component]
fn MetricCard(card: ComparisonCard, on_inspect: EventHandler<EvidenceRequest>) -> Element {
    let metric = card.metric();
    let distribution = &card.result().report().metrics()[card.index()];
    let (left, right) = distribution.averages();
    let (low, high) = distribution.range();
    let (left_count, right_count) = distribution.counts();
    let (left_population, right_population) = card.result().report().counts();
    let inspectable = card.result().plan().unit() == ObservationUnit::Episodes
        && card.result().plan().dataset().is_some();
    let delta = MetricNumber(left.zip(right).map(|(left, right)| left - right)).display();
    rsx! {
        section { class: "comparison-metric-card", "aria-label": metric.label(),
            header {
                h2 { "{metric.label()}" }
                span { class: "comparison-delta", title: "Workflow A mean minus Workflow B mean", "Δ {delta} {metric.unit()}" }
            }
            p { class: "comparison-measurement-description", "{metric.description()}" }
            div { class: "comparison-averages",
                div {
                    span { class: "comparison-series-label is-selected", "A · Mean" }
                    strong { "{MetricNumber(left).display()}" small { " {metric.unit()}" } }
                }
                div {
                    span { class: "comparison-series-label", "B · Mean" }
                    strong { "{MetricNumber(right).display()}" small { " {metric.unit()}" } }
                }
            }
            if left_count==0 && right_count==0 {
                p { class: "comparison-no-values", "No measured values." }
            } else {
                div { class: "comparison-histogram", "aria-label": "{metric.label()} distribution",
                    for bucket in 0..10 {
                        {
                            let (left_height,right_height)=distribution.heights(bucket);
                            let (left_bin,right_bin)=distribution.bin_counts(bucket);
                            let label=format!("{} {}: A {left_bin}, B {right_bin}",distribution.bin_label(bucket),metric.unit());
                            let result=card.result().clone();
                            let index=card.index();
                            rsx! {
                                button {
                                    class: "comparison-bin", disabled: !inspectable || (left_bin==0 && right_bin==0),
                                    title: label.clone(), "aria-label": format!("Inspect {label}"),
                                    onclick: move |_|on_inspect.call(EvidenceRequest{
                                        result:result.clone(), selection:ComparisonEvidence::new(if left_bin>0 {WorkflowSide::Left}else{WorkflowSide::Right}).with_bin(Some((index,bucket)))
                                    }),
                                    span { class: "comparison-bar is-selected", style: "height: {left_height}%;" }
                                    span { class: "comparison-bar is-baseline", style: "height: {right_height}%;" }
                                }
                            }
                        }
                    }
                }
                div { class: "comparison-axis",
                    span { "{MetricNumber(Some(low)).display()} {metric.unit()}" }
                    span { "{MetricNumber(Some(high)).display()} {metric.unit()}" }
                }
            }
            footer {
                span { "Measured: A {left_count}/{left_population} · B {right_count}/{right_population}" }
                details {
                    summary { "Measurement details" }
                    p { "{metric.source()}" }
                    p { "Bars show the share of measured {card.result().plan().unit().label()} on a shared scale. Missing values are excluded from means." }
                }
            }
        }
    }
}

/// Every measurement that could not be calculated, in one card: the names as
/// a list and each distinct error once, instead of one apologetic card per
/// metric repeating the same message.
#[component]
fn UnavailableMeasurements(failures: Vec<(DashboardMetric, String)>) -> Element {
    let mut errors = Vec::<&String>::new();
    for (_, error) in &failures {
        if !errors.contains(&error) {
            errors.push(error);
        }
    }
    rsx! {
        section { class: "comparison-metric-card comparison-failed-card", "aria-label": "Measurements that could not be calculated",
            header {
                h2 { if failures.len() == 1 { "1 measurement unavailable" } else { "{failures.len()} measurements unavailable" } }
            }
            p { class: "comparison-measurement-description", "These could not be calculated from the recorded signals. Compare again to retry them." }
            ul { class: "comparison-failed-list",
                for (metric, _) in &failures {
                    li { key: "failed-{metric.column()}", "{metric.label()}" }
                }
            }
            details { class: "comparison-failed-errors",
                summary { if errors.len() == 1 { "Error" } else { "Errors · {errors.len()}" } }
                for (index, error) in errors.iter().enumerate() {
                    p { key: "{index}", "{error}" }
                }
            }
        }
    }
}

#[component]
fn PendingMetricCard(metric: DashboardMetric, running: bool) -> Element {
    rsx! {
        section { class: "comparison-metric-card is-pending", "aria-label": if running { format!("Calculating {}",metric.label()) } else { format!("Queued {}",metric.label()) }, "aria-busy": "true",
            header { h2 { "{metric.label()}" } span { class: "comparison-card-state", if running { "Calculating" } else { "Queued" } } }
            p { class: "comparison-measurement-description", "{metric.description()}" }
            div { class: "comparison-pending-body",
                div { class: "comparison-placeholder", "aria-hidden": "true", span {} span {} }
                p { if running { "Reading measurements and building distributions…" } else { "This card will start when the preceding work finishes." } }
            }
        }
    }
}
