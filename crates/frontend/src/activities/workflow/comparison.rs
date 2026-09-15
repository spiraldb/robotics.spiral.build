//! Dashboard presentation of a completed output from the existing workflow run.
use super::state::WorkflowController;
use crate::activities::data::DatasetClient;
use crate::components::interaction::BrowserFocus;
use dioxus::prelude::*;
use execution::CompletedComparison;
use report::{MetricDistribution, MetricNumber};
use spiral_robotics_common::workflow::{
    WorkflowRunPhase, WorkflowSpec,
    comparison::{
        ComparisonBaseline, ComparisonEvidence, ComparisonMetric, EpisodeComparison, EvidenceCohort,
    },
};
mod evidence;
mod execution;
mod report;

#[component]
pub(super) fn WorkflowComparison(
    controller: WorkflowController,
    on_author: EventHandler,
) -> Element {
    let definition = controller.spec();
    let snapshot = controller.snapshot();
    let runs = controller.workflow_runs();
    let output = use_memo(move || {
        controller
            .visible_run()
            .filter(|run| run.snapshot().phase() == WorkflowRunPhase::Completed)
            .map(|run| CompletedComparison::from_run(&run))
    });
    let mut evidence = use_signal(ComparisonEvidence::default);
    let mut inspect = use_signal(|| None::<serde_json::Map<String, serde_json::Value>>);
    let mut displayed_run = use_signal(|| None::<String>);
    use_effect(move || {
        let run = controller.snapshot().run_id().map(str::to_owned);
        if *displayed_run.peek() != run {
            displayed_run.set(run);
            evidence.set(ComparisonEvidence::default());
            inspect.set(None);
        }
    });
    let inspecting = inspect.read().is_some();
    let previous = output.read().clone();
    let stale = snapshot.result_stale();
    rsx! {
        section { class: "episode-comparison", "aria-label": "Episode comparison",
            if let Some(config) = definition.comparison().cloned() {
                aside { class: "comparison-sidebar",
                    div { class: "comparison-sidebar-heading", h2 { "Dashboard" } span { "{config.dataset().label()}" } }
                    MetricEditor { controller, configuration: config.clone() }
                    div { class: "comparison-sidebar-note",
                        h3 { "Workflow selection" }
                        p { "{config.description()}" }
                        p { class: "comparison-selection-rule", "{config.criterion().column()} {config.criterion().operator().label()} {config.criterion().threshold()}" }
                        button { class: "comparison-text-button", onclick: move |_| on_author.call(()), "Edit workflow" }
                    }
                    p { class: "comparison-save-note", "Selection comes from the workflow. Use its Run action to publish updated results. Save stores the workflow and dashboard settings." }
                }
                main { class: "comparison-content",
                    div { class: "comparison-output-source",
                        span { "Workflow output" }
                        if !runs.is_empty() {
                            select { "aria-label": "Comparison workflow run", value: snapshot.run_id().unwrap_or_default(), onchange: move |event| controller.select_run(event.value()),
                                for (index, run) in runs.iter().enumerate() {
                                    option { value: run.snapshot().run_id().unwrap_or_default(), selected: run.snapshot().run_id() == snapshot.run_id(), "Run {index + 1} · {run.snapshot().phase():?}" }
                                }
                            }
                        }
                        button { class: "comparison-text-button", onclick: move |_| on_author.call(()), "Edit workflow" }
                    }
                    if snapshot.phase().active() { div { class: "comparison-progress", role: "status", span {} "The workflow is running. Its comparison output will appear here." } }
                    if let Some(message) = snapshot.error() { div { class: "comparison-error", role: "alert", "{message}" } }
                    if stale { div { class: "comparison-stale", role: "status", "The workflow changed. Run it to update this comparison output." } }
                    match previous {
                        Some(Ok(result)) => rsx! {
                            if let Some(row) = inspect() {
                                div { class: "comparison-inspector",
                                    crate::activities::data::ComparisonEpisodeInspector { dataset: result.configuration().dataset().clone(), row, on_back: move |_| inspect.set(None) }
                                }
                            }
                            div { class: if inspecting { "workflow-mode-hidden" } else { "comparison-review" },
                                ComparisonDashboard { result: result.clone(), stale, controller,
                                    on_select: move |selection| { evidence.set(selection); BrowserFocus::after_render("comparison-evidence-heading".into()); },
                                }
                                if !stale { evidence::ComparisonEpisodes { result, selection: evidence(), on_select: move |selection| evidence.set(selection), on_inspect: move |row| inspect.set(Some(row)) } }
                            }
                        },
                        Some(Err(message)) => rsx! { p { class: "comparison-error", role: "alert", "{message}" } },
                        None if !snapshot.phase().active() => rsx! {
                            div { class: "comparison-welcome", h2 { "Compare the workflow’s output." } p { "Run this workflow to calculate its selected population and full-dataset distributions. The same run appears in Build and Compare." } }
                        },
                        _ => rsx! {},
                    }
                }
            } else {
                div { class: "comparison-setup",
                    h1 { "Start with a selection workflow." }
                    p { "Ask the Workflow Assistant to select episodes and publish a comparison output. Its graph, Run action, and results will drive this dashboard." }
                    button { class: "comparison-primary", onclick: move |_| on_author.call(()), "Build with Assistant" }
                }
            }
        }
    }
}

/// Selection is authored by the existing Assistant and shown with its SQL.
#[component]
pub(super) fn WorkflowOutputDefinition(controller: WorkflowController) -> Element {
    let spec = controller.spec();
    rsx! {
        section { class: "workflow-output-definition",
            h3 { "Workflow output" }
            if let Some(output) = spec.comparison() {
                strong { "Episode selection + distributions" }
                p { "{output.description()}" }
                p { "{output.dataset().label()} · {output.criterion().column()} {output.criterion().operator().label()} {output.criterion().threshold()}" }
                p { "The graph includes selection and full-population aggregates. Run publishes the output to Results and Compare." }
            } else { p { "SQL rows. Ask the Assistant to add an episode selection and comparison output." } }
            button { class: "comparison-text-button", onclick: move |_| controller.activate_panel(super::state::ASSISTANT_PANEL_ID.into()), "Edit with Assistant" }
        }
    }
}

#[component]
fn MetricEditor(controller: WorkflowController, configuration: EpisodeComparison) -> Element {
    let mut expanded = use_signal(|| false);
    let mut column = use_signal(String::new);
    let mut label = use_signal(String::new);
    let mut unit = use_signal(String::new);
    let add_config = configuration.clone();
    rsx! {
        div { class: "comparison-metric-editor",
            h3 { "Dashboard metrics" }
            for (index, metric) in configuration.metrics().iter().enumerate() {
                div { class: "comparison-metric-row", key: "{metric.column()}",
                    span { "{metric.label()}" }
                    button { class: "comparison-icon-button", "aria-label": "Move {metric.label()} up", disabled: index == 0,
                        onclick: { let mut config = configuration.clone(); move |_| { let mut metrics = config.metrics().to_vec(); metrics.swap(index, index - 1); config.set_metrics(metrics); controller.set_comparison(config.clone()); } }, crate::components::icons::ArrowUpIcon {} }
                    button { class: "comparison-icon-button", "aria-label": "Remove {metric.label()}", disabled: configuration.metrics().len() == 1,
                        onclick: { let mut config = configuration.clone(); move |_| { let mut metrics = config.metrics().to_vec(); metrics.remove(index); config.set_metrics(metrics); controller.set_comparison(config.clone()); } }, crate::components::interaction::CloseIcon {} }
                }
            }
            button { class: "comparison-text-button", disabled: configuration.metrics().len() >= 6, "aria-expanded": expanded(), onclick: move |_| expanded.toggle(), "Add metric" }
            if expanded() {
                div { class: "comparison-add-metric",
                    input { "aria-label": "Metric column", placeholder: "Numeric SQL column", value: column(), oninput: move |event| column.set(event.value()) }
                    input { "aria-label": "Metric label", placeholder: "Display name", value: label(), oninput: move |event| label.set(event.value()) }
                    input { "aria-label": "Metric unit", placeholder: "Unit (optional)", value: unit(), oninput: move |event| unit.set(event.value()) }
                    button { class: "comparison-secondary", disabled: column().trim().is_empty() || label().trim().is_empty() || configuration.metrics().iter().any(|metric| metric.column() == column().trim()), onclick: move |_| {
                        let mut config = add_config.clone(); let mut metrics = config.metrics().to_vec();
                        metrics.push(ComparisonMetric::new(column().trim(), label().trim(), unit().trim())); config.set_metrics(metrics); controller.set_comparison(config);
                        column.set(String::new()); label.set(String::new()); unit.set(String::new()); expanded.set(false);
                    }, "Add to dashboard" }
                }
            }
        }
    }
}

#[component]
fn ComparisonDashboard(
    result: CompletedComparison,
    stale: bool,
    controller: WorkflowController,
    on_select: EventHandler<ComparisonEvidence>,
) -> Element {
    let population = result.report().population();
    let visible_metrics = controller
        .spec()
        .comparison()
        .map(|config| config.metrics().to_vec())
        .unwrap_or_else(|| result.configuration().metrics().to_vec());
    let baseline_id = controller
        .spec()
        .comparison()
        .and_then(|config| config.baseline_workflow().map(str::to_owned));
    let reference_run = baseline_id
        .as_deref()
        .and_then(|id| controller.latest_output(id));
    let reference = baseline_id.as_ref().map(|_| {
        reference_run
            .as_ref()
            .ok_or_else(|| "Run the baseline workflow to publish its output.".to_owned())
            .and_then(CompletedComparison::from_run)
    });
    let baseline_label = baseline_id
        .as_ref()
        .map(|id| {
            controller
                .workflows()
                .iter()
                .find(|workflow| workflow.id() == id)
                .map(|workflow| workflow.name().to_owned())
                .unwrap_or_else(|| id.clone())
        })
        .unwrap_or_else(|| "All episodes".into());
    let baseline = ComparisonBaseline::All;
    let fraction = if population.all() == 0 {
        0.0
    } else {
        population.selected() as f64 / population.all() as f64 * 100.0
    };
    rsx! {
        header { class: "comparison-dashboard-heading",
            div { h1 { "Episode comparison" } p { "{result.configuration().description()}" } }
            label { "Baseline workflow"
                select { "aria-label": "Baseline workflow", value: baseline_id.clone().unwrap_or_default(), onchange: move |event| {
                    if let Some(mut config) = controller.spec().comparison().cloned() { config.set_baseline_workflow((!event.value().is_empty()).then(|| event.value())); controller.set_comparison(config); }
                },
                    option { value: "", selected: baseline_id.is_none(), "All episodes (default)" }
                    for workflow in controller.workflows().iter().filter(|workflow| workflow.id() != controller.spec().id()) {
                        option { value: workflow.id(), selected: baseline_id.as_deref() == Some(workflow.id()), "{workflow.name()}" }
                    }
                }
            }
        }
        div { class: "comparison-population", "aria-label": "Population summary",
            button { class: "comparison-population-selected", disabled: stale, onclick: move |_| on_select.call(ComparisonEvidence::new(EvidenceCohort::Selected)),
                span { "Selected episodes" }
                strong { "{population.selected()}" small { " / {population.all()}" } }
                span { "{fraction:.1}% of dataset · {population.selected_hours():.2} h" }
            }
            div { class: "comparison-population-context",
                div { class: "comparison-population-track", title: "{population.selected()} selected, {population.remaining()} remaining, {population.unknown()} unknown",
                    span { style: "width: {fraction}%;", class: "is-selected" }
                    span { style: format!("width: {}%;", if population.all() == 0 { 0.0 } else { population.remaining() as f64 / population.all() as f64 * 100.0 }), class: "is-remaining" }
                }
                div { class: "comparison-population-labels",
                    button { disabled: stale, onclick: move |_| on_select.call(ComparisonEvidence::new(EvidenceCohort::Remaining)), span { class: "comparison-dot is-remaining" } "{population.remaining()} remaining" }
                    button { disabled: stale, onclick: move |_| on_select.call(ComparisonEvidence::new(EvidenceCohort::Unknown)), span { class: "comparison-dot is-unknown" } "{population.unknown()} unknown" }
                    span { "{population.hours():.2} recording hours" }
                }
            }
        }
        div { class: "comparison-chart-heading",
            h2 { "How the populations differ" }
            div { class: "comparison-legend", span { class: "comparison-dot is-selected" } "Selected" span { class: "comparison-dot is-baseline" } "{baseline_label}" }
        }
        p { class: "comparison-chart-hint", "Select a bar to inspect matching episodes." }
        if let Some(run) = &reference_run {
            p { class: "comparison-chart-hint", "Baseline: latest completed output · {run.snapshot().run_id().unwrap_or_default()}" }
        }
        div { class: "comparison-metric-grid",
            for metric in visible_metrics.iter() {
                if let Some((index, distribution)) = result.metric_index(metric.column()).and_then(|index| result.report().metrics().get(index).map(|distribution| (index, distribution))) {
                    match reference.as_ref() {
                        None => rsx! { MetricPanel { metric: metric.clone(), distribution: distribution.clone(), baseline, baseline_label: baseline_label.clone(), stale, on_bin: move |bin| on_select.call(ComparisonEvidence::default().with_bin(Some((index, bin)))) } },
                        Some(reference) => match reference.as_ref().map_err(Clone::clone).and_then(|reference| result.against(reference, metric)) {
                            Ok(distribution) => rsx! { MetricPanel { metric: metric.clone(), distribution, baseline, baseline_label: baseline_label.clone(), stale, on_bin: move |bin| on_select.call(ComparisonEvidence::default().with_bin(Some((index, bin)))) } },
                            Err(message) => rsx! { section { class: "comparison-metric-panel", h3 { "{metric.label()}" } p { class: "comparison-no-values", "{message}" } } },
                        },
                    }
                }
            }
        }
        footer { class: "comparison-provenance",
            span { {format!("Full dataset · {} episodes · {:.1} s", population.all(), result.elapsed_ms() as f64 / 1000.0)} }
            details { summary { "Executed SQL" } pre { "{result.sql()}" } }
        }
    }
}

#[component]
fn MetricPanel(
    metric: ComparisonMetric,
    distribution: MetricDistribution,
    baseline: ComparisonBaseline,
    baseline_label: String,
    stale: bool,
    on_bin: EventHandler<usize>,
) -> Element {
    let (low, high) = distribution.range();
    rsx! {
        section { class: "comparison-metric-panel",
            header { h3 { "{metric.label()}" } span { class: "comparison-delta", title: "Selected mean minus baseline mean.", "Δ {distribution.delta(baseline)} {metric.unit()}" } }
            div { class: "comparison-averages",
                div { span { "Selected mean" } strong { class: "is-selected", "{MetricNumber(distribution.selected_average()).display()}" small { " {metric.unit()}" } } }
                div { span { "{baseline_label} mean" } strong { "{MetricNumber(distribution.baseline_average(baseline)).display()}" small { " {metric.unit()}" } } }
            }
            if distribution.bins().is_empty() { p { class: "comparison-no-values", "No measured values for this metric." } }
            else {
                div { class: "comparison-histogram", "aria-label": "{metric.label()} distribution",
                    for bin in 0..10 {
                        {
                            let (selected, reference) = distribution.height(bin, baseline);
                            let counts = distribution.bins().iter().find(|row| row.bucket() == bin);
                            let selected_n = counts.map_or(0, |row| row.selected());
                            let baseline_n = counts.map_or(0, |row| row.baseline(baseline));
                            let label = format!("{} {}: {} selected, {} baseline episodes", distribution.bin_label(bin), metric.unit(), selected_n, baseline_n);
                            rsx! { button { class: "comparison-bin", disabled: stale || selected_n == 0, title: "{label}", "aria-label": "Inspect {label}", onclick: move |_| on_bin.call(bin),
                                span { class: "comparison-bar is-baseline", style: "height: {reference}%;" }
                                span { class: "comparison-bar is-selected", style: "height: {selected}%;" }
                            } }
                        }
                    }
                }
                div { class: "comparison-axis", span { "{MetricNumber(Some(low)).display()} {metric.unit()}" } span { "{MetricNumber(Some(high)).display()} {metric.unit()}" } }
            }
            p { class: "comparison-chart-footnote", "Measured: {distribution.selected_count()} selected · {distribution.baseline_count(baseline)} baseline. Bars show share of measured episodes." }
        }
    }
}
