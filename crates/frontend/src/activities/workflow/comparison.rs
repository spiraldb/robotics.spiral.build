//! Progressive comparisons of ordinary workflow outputs and recorded signals.
mod agent;
mod dashboard;
mod discovery;
mod evidence;
mod recorded_signal;
mod report;
mod selection;

use super::state::WorkflowController;
use crate::components::interaction::BrowserFocus;
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

#[component]
pub(super) fn WorkflowComparison(controller: WorkflowController) -> Element {
    let mut left = use_signal(String::new);
    use_effect(move || {
        let current = controller.spec();
        if left().is_empty()
            && controller
                .workflows()
                .iter()
                .any(|workflow| workflow.id() == current.id())
        {
            left.set(current.id().to_owned());
        }
    });
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
            header { class: "comparison-pair-header",
                h1 { "Compare workflows" }
                div { class: "comparison-pair-controls",
                    label {
                        "Workflow A"
                        select {
                            "aria-label": "Workflow A", value: left(), disabled: busy,
                            onchange: move |event| {
                                left.set(event.value());
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
                    label {
                        "Workflow B"
                        select {
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
                    button {
                        class: "comparison-primary", disabled: busy || !valid_left,
                        onclick: move |_| {
                            let selected_right=right();
                            let ids=(left(),(!selected_right.is_empty()).then_some(selected_right));
                            dashboard.write().start();
                            evidence_request.set(None);
                            inspected.set(None);
                            spawn(async move {
                                let outcome=ComparisonAgent::compare(ids,move |event| dashboard.write().accept(event)).await;
                                dashboard.write().finish(outcome);
                            });
                        },
                        if busy { "Analyzing…" } else { "Compare" }
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
                if let Some(status)=current.status() {
                    div { class: "comparison-analysis-status", role: "status",
                        span { class: "comparison-status-dot" }
                        span { "{status}…" }
                        if !current.cards().is_empty() { span { class: "comparison-ready-count", "{current.cards().len()} ready · {current.pending().len()} running" } }
                    }
                }
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
                    if current.population().is_some() {
                        div { class: "comparison-card-grid",
                            for card in current.cards().iter().filter(|card|card.shown()) {
                                MetricCard { card: card.clone(), on_inspect: inspect }
                            }
                            for metric in current.pending() { PendingMetricCard { key: "{metric.column()}", metric: metric.clone() } }
                            for (metric, error) in current.failures() {
                                section { class: "comparison-metric-card comparison-failed-card", key: "failed-{metric.column()}",
                                    h2 { "{metric.label()}" }
                                    p { "This measurement could not be calculated. Compare again to retry." }
                                    details { summary { "Error details" } p { "{error}" } }
                                }
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
                    } else if busy {
                        div { class: "comparison-empty",
                            p { "Establishing the two populations. Completed comparisons will appear here." }
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

#[component]
fn PendingMetricCard(metric: DashboardMetric) -> Element {
    rsx! {
        section { class: "comparison-metric-card is-pending", "aria-label": "Analyzing {metric.label()}",
            header { h2 { "{metric.label()}" } }
            p { class: "comparison-measurement-description", "{metric.description()}" }
            div { class: "comparison-pending-body", "Calculating from recorded values…" }
        }
    }
}
