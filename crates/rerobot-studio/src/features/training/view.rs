//! Curve-first policy training workspace.
//!
//! This surface is intentionally a browser over Rust-owned state. It starts and
//! stops runs through the backend, then renders only metrics received from the
//! Studio event stream; no training lifecycle is inferred in the browser.

use dioxus::prelude::*;
use rerobot_core::api::{TrainingMetricPoint, TrainingReadiness, TrainingRun, TrainingRunPhase};
use rerobot_panels::{LayoutNode, Panel, PanelLayout, PanelWorkspace, SplitAxis};

use crate::app::state::studio;
use crate::features::training::controller::training_controller;

const CHART_WIDTH: f64 = 1_000.0;
const CHART_TOP: f64 = 18.0;
const CHART_BOTTOM: f64 = 242.0;

#[component]
pub(crate) fn TrainingWorkspace(dataset: String) -> Element {
    let studio = studio();
    let controller = training_controller();
    let snapshot = studio.training.read().clone();
    let active_run = snapshot.active_run().cloned();
    let viewed_run = controller
        .selected_run()
        .as_ref()
        .and_then(|id| snapshot.runs.iter().find(|run| &run.id == id))
        .or(active_run.as_ref())
        .or_else(|| snapshot.runs.first())
        .cloned();

    let workbench = Panel::new(
        "training-workbench",
        "Training",
        "training-workbench",
        rsx! {
            TrainingRunBuilder {
                dataset: dataset.clone(),
                readiness: snapshot.readiness.clone(),
                active_run: active_run.clone(),
                viewed_run: viewed_run.clone(),
                history: snapshot.runs.clone(),
            }
        },
    )
    .with_class("training-workbench-editor");
    let learning = Panel::new(
        "training-learning",
        "Learning curves",
        "training-learning",
        rsx! { TrainingLearning { run: viewed_run.clone() } },
    )
    .with_class("training-learning-editor");
    let data_path = Panel::new(
        "training-data-path",
        "Data path",
        "training-data",
        rsx! { TrainingDataPath { dataset: dataset.clone(), run: viewed_run.clone() } },
    )
    .with_class("training-data-editor");
    let resources = Panel::new(
        "training-resources",
        "Resources",
        "training-resources",
        rsx! { TrainingResources { run: viewed_run } },
    )
    .with_class("training-resource-editor");
    let panels = vec![workbench, learning, data_path, resources];

    let top = LayoutNode::split(
        "training-upper",
        SplitAxis::Horizontal,
        0.76,
        LayoutNode::tile("training-learning", ["training-learning".to_owned()]),
        LayoutNode::tile("training-data", ["training-data-path".to_owned()]),
    );
    let visualization = LayoutNode::split(
        "training-rows",
        SplitAxis::Vertical,
        0.62,
        top,
        LayoutNode::tile("training-resources", ["training-resources".to_owned()]),
    );
    let layout = PanelLayout::new(LayoutNode::split(
        "training-columns",
        SplitAxis::Horizontal,
        0.29,
        LayoutNode::tile("training-workbench", ["training-workbench".to_owned()]),
        visualization,
    ));

    rsx! {
        PanelWorkspace {
            panels,
            initial_layout: layout,
            storage_key: Some("rerobot.panel-layout.v1.training".to_owned()),
            on_layout_change: move |_layout: PanelLayout| {},
            on_panel_activate: move |_id| {},
            on_resize: move |_| {},
        }
    }
}

#[component]
fn TrainingRunBuilder(
    dataset: String,
    readiness: TrainingReadiness,
    active_run: Option<TrainingRun>,
    viewed_run: Option<TrainingRun>,
    history: Vec<TrainingRun>,
) -> Element {
    let studio = studio();
    let controller = training_controller();
    let request = controller.request();
    let selected_run = controller.selected_run();
    let starting = controller.starting();
    let stopping = controller.stopping();
    let can_start = readiness.ready && active_run.is_none() && !starting;
    let active_id = active_run.as_ref().map(|run| run.id.clone());
    let progress = active_run.as_ref().map(|run| {
        if run.config.steps == 0 {
            0.0
        } else {
            run.current_step as f64 / run.config.steps as f64 * 100.0
        }
    });

    rsx! {
        div { class: "training-workbench studio-workbench-scroll",
            header { class: "training-run-header",
                div {
                    h2 { "Train a policy" }
                    p { "Spiral streams in. Checkpoints return to Spiral." }
                }
                span { class: "training-policy-chip", "ACT · state" }
            }

            if !readiness.ready {
                div { class: "training-inline-state training-inline-state-caution",
                    strong { "Training is not ready" }
                    span { "{readiness.summary}" }
                }
            }

            if let Some(run) = active_run.clone() {
                section { class: "training-active-run",
                    div { class: "training-active-heading",
                        div {
                            strong { "{run.config.run_name}" }
                            span { class: phase_class(run.phase), "{run.phase.label()}" }
                        }
                        span { class: "training-step-readout", "{run.current_step} / {run.config.steps}" }
                    }
                    div { class: "training-progress-track", "aria-label": "Training progress",
                        div { class: "training-progress-fill", style: "width: {progress.unwrap_or_default().clamp(0.0, 100.0):.2}%" }
                    }
                    if let Some(message) = run.message {
                        p { "{message}" }
                    }
                    small { class: "training-stop-consequence",
                        "Finishes the current optimizer step and publishes a final checkpoint. The backend force-stops the worker after 10 seconds."
                    }
                    button {
                        r#type: "button",
                        class: "training-stop-button",
                        disabled: run.phase == TrainingRunPhase::Stopping || stopping.is_some(),
                        onclick: move |_| {
                            let id = active_id.clone().unwrap_or_default();
                            controller.stop(studio, id);
                        },
                        if run.phase == TrainingRunPhase::Stopping || stopping.is_some() { "Stopping…" } else { "Stop after current step" }
                    }
                }
            }

            if let Some(run) = viewed_run.as_ref().filter(|run| !run.phase.active()) {
                section {
                    class: if run.phase == TrainingRunPhase::Failed { "training-run-result training-run-result-failed" } else { "training-run-result" },
                    role: if run.phase == TrainingRunPhase::Failed { "alert" } else { "status" },
                    h3 { "{run.phase.label()} · {run.config.run_name}" }
                    p { {run.message.clone().unwrap_or_else(|| "The run ended without a status message.".to_owned())} }
                    if !run.logs.is_empty() {
                        details { open: run.phase == TrainingRunPhase::Failed,
                            summary { "Worker log · {run.logs.len()} lines" }
                            pre {
                                for line in run.logs.iter().rev().take(12).rev() {
                                    code { "{line}\n" }
                                }
                            }
                        }
                    }
                }
            }

            section { class: "training-form-section",
                label { class: "training-field training-field-wide",
                    span { "Run name" }
                    input {
                        value: request.run_name.clone(),
                        disabled: active_run.is_some(),
                        oninput: move |event| {
                            let value = event.value();
                            controller.edit_request(move |request| request.run_name = value);
                        },
                    }
                }
                div { class: "training-form-grid",
                    label { class: "training-field",
                        span { "Device" }
                        select {
                            value: request.device.clone(),
                            disabled: active_run.is_some(),
                            onchange: move |event| {
                                let value = event.value();
                                controller.edit_request(move |request| request.device = value);
                            },
                            option { value: "auto", "Auto" }
                            option { value: "cuda", "CUDA" }
                            option { value: "mps", "Apple MPS" }
                            option { value: "cpu", "CPU" }
                        }
                    }
                    label { class: "training-field",
                        span { "Steps" }
                        input {
                            r#type: "number", min: "1", step: "100",
                            value: "{request.steps}",
                            disabled: active_run.is_some(),
                            oninput: move |event| if let Ok(value) = event.value().parse() {
                                controller.edit_request(move |request| request.steps = value);
                            },
                        }
                    }
                    label { class: "training-field",
                        span { "Batch" }
                        input {
                            r#type: "number", min: "1",
                            value: "{request.batch_size}",
                            disabled: active_run.is_some(),
                            oninput: move |event| if let Ok(value) = event.value().parse() {
                                controller.edit_request(move |request| request.batch_size = value);
                            },
                        }
                    }
                    label { class: "training-field",
                        span { "Action chunk" }
                        input {
                            r#type: "number", min: "1", max: "256",
                            value: "{request.chunk_size}",
                            disabled: active_run.is_some(),
                            oninput: move |event| if let Ok(value) = event.value().parse() {
                                controller.edit_request(move |request| request.chunk_size = value);
                            },
                        }
                    }
                    label { class: "training-field",
                        span { "Learning rate" }
                        input {
                            r#type: "number", min: "0", step: "0.00001",
                            value: "{request.learning_rate}",
                            disabled: active_run.is_some(),
                            oninput: move |event| if let Ok(value) = event.value().parse() {
                                controller.edit_request(move |request| request.learning_rate = value);
                            },
                        }
                    }
                    label { class: "training-field",
                        span { "Checkpoint every" }
                        input {
                            r#type: "number", min: "1", step: "100",
                            value: "{request.checkpoint_every}",
                            disabled: active_run.is_some(),
                            oninput: move |event| if let Ok(value) = event.value().parse() {
                                controller.edit_request(move |request| request.checkpoint_every = value);
                            },
                        }
                    }
                }
            }

            section { class: "training-query-section",
                header {
                    div {
                        h3 { "Spiral sample query" }
                        p { "Must return equal-width state and action vectors." }
                    }
                    span { "{dataset}" }
                }
                textarea {
                    spellcheck: "false",
                    value: request.sql.clone(),
                    disabled: active_run.is_some(),
                    oninput: move |event| {
                        let value = event.value();
                        controller.edit_request(move |request| request.sql = value);
                    },
                }
                div { class: "training-column-row",
                    label { class: "training-field",
                        span { "State column" }
                        input {
                            value: request.state_column.clone(),
                            disabled: active_run.is_some(),
                            oninput: move |event| {
                                let value = event.value();
                                controller.edit_request(move |request| request.state_column = value);
                            },
                        }
                    }
                    label { class: "training-field",
                        span { "Action column" }
                        input {
                            value: request.action_column.clone(),
                            disabled: active_run.is_some(),
                            oninput: move |event| {
                                let value = event.value();
                                controller.edit_request(move |request| request.action_column = value);
                            },
                        }
                    }
                }
            }

            button {
                r#type: "button",
                class: "training-start-button",
                disabled: !can_start,
                title: if active_run.is_some() { "Stop the active run first".to_owned() } else if !readiness.ready { readiness.summary.clone() } else { "Start local PyTorch training".to_owned() },
                onclick: move |_| {
                    controller.start(studio);
                },
                if starting { "Starting…" } else { "Start training" }
            }

            section { class: "training-history",
                header {
                    h3 { "Run history" }
                    span { "{history.len()} backend records" }
                }
                if history.is_empty() {
                    p { class: "training-history-empty", "Completed and failed runs will remain here." }
                }
                for run in history {
                    button {
                        r#type: "button",
                        class: if selected_run.as_deref() == Some(run.id.as_str()) { "training-history-row training-history-row-selected" } else { "training-history-row" },
                        onclick: {
                            let id = run.id.clone();
                            move |_| controller.select_run(id.clone())
                        },
                        span { class: "training-history-name", "{run.config.run_name}" }
                        span { class: phase_class(run.phase), "{run.phase.label()}" }
                        span { class: "training-history-step", "{run.current_step}" }
                    }
                }
            }
        }
    }
}

#[component]
fn TrainingLearning(run: Option<TrainingRun>) -> Element {
    let Some(run) = run else {
        return rsx! {
            div { class: "training-panel-empty",
                strong { "No learning curve yet" }
                span { "Start a run to plot loss against the shared training-step axis." }
            }
        };
    };
    let (train_points, validation_points, loss_top, loss_bottom) = loss_points(&run);
    let last = run.metrics.last();
    let latest_train = last.map(|point| format_loss(point.train_loss));
    let latest_validation = run
        .metrics
        .iter()
        .rev()
        .find_map(|point| point.validation_loss)
        .map(format_loss);
    let lr = last.map(|point| format!("{:.2e}", point.learning_rate));
    let latest_train = latest_train.unwrap_or_else(|| "—".to_owned());
    let latest_validation = latest_validation.unwrap_or_else(|| "—".to_owned());
    let lr = lr.unwrap_or_else(|| "—".to_owned());

    rsx! {
        section { class: "training-learning",
            header { class: "training-panel-heading",
                div {
                    h2 { "{run.config.run_name}" }
                    p { "{run.config.policy.label()} · {run.config.device} · step {run.current_step} of {run.config.steps}" }
                }
                span { class: phase_class(run.phase), "{run.phase.label()}" }
            }
            div { class: "training-loss-summary",
                div {
                    span { class: "training-series-dot training-series-train" }
                    small { "Train" }
                    strong { "{latest_train}" }
                }
                div {
                    span { class: "training-series-dot training-series-validation" }
                    small { "Validation" }
                    strong { "{latest_validation}" }
                }
                div {
                    small { "Learning rate" }
                    strong { "{lr}" }
                }
            }
            div { class: "training-chart-wrap",
                if run.metrics.is_empty() {
                    div { class: "training-chart-empty",
                        if run.phase == TrainingRunPhase::Failed {
                            "No optimizer metrics were produced before this run failed."
                        } else if run.phase.active() {
                            "Waiting for the first optimizer step…"
                        } else {
                            "This run ended before producing optimizer metrics."
                        }
                    }
                }
                svg {
                    class: "training-loss-chart",
                    view_box: "0 0 1000 260",
                    preserve_aspect_ratio: "none",
                    "aria-label": "Training and validation loss by optimizer step",
                    for y in [18, 74, 130, 186, 242] {
                        line { x1: "0", x2: "1000", y1: "{y}", y2: "{y}", class: "training-grid-line" }
                    }
                    for x in [0, 250, 500, 750, 1000] {
                        line { x1: "{x}", x2: "{x}", y1: "18", y2: "242", class: "training-grid-line" }
                    }
                    if !train_points.is_empty() {
                        polyline { points: train_points, class: "training-line training-line-train" }
                    }
                    if !validation_points.is_empty() {
                        polyline { points: validation_points, class: "training-line training-line-validation" }
                    }
                }
                div { class: "training-chart-y-labels",
                    span { "{loss_top}" }
                    span { "{loss_bottom}" }
                }
                div { class: "training-chart-x-labels",
                    span { "0" }
                    span { "{run.config.steps / 2}" }
                    span { "{run.config.steps}" }
                }
            }
            div { class: "training-event-strip",
                span { "Events" }
                div { class: "training-event-track",
                    i { class: "training-event-axis" }
                    for checkpoint in &run.checkpoints {
                        span {
                            class: "training-event-tick training-event-checkpoint",
                            style: "left: {step_percent(checkpoint.step, run.config.steps):.3}%",
                            title: "Checkpoint {checkpoint.step} published to Spiral",
                            "aria-label": "Checkpoint at step {checkpoint.step}",
                        }
                    }
                    for point in run.metrics.iter().filter(|point| point.event.is_some()) {
                        span {
                            class: if point.event.as_deref().is_some_and(|event| event.contains("loader stall")) { "training-event-tick training-event-stall" } else { "training-event-tick training-event-validation" },
                            style: "left: {step_percent(point.step, run.config.steps):.3}%",
                            title: metric_event_label(point),
                            "aria-label": metric_event_label(point),
                        }
                    }
                }
                div { class: "training-event-legend",
                    small { "Validation" }
                    small { "Checkpoint" }
                    small { "Loader stall" }
                }
            }
        }
    }
}

#[component]
fn TrainingDataPath(dataset: String, run: Option<TrainingRun>) -> Element {
    let transfer = run
        .as_ref()
        .and_then(|run| run.metrics.last())
        .map(|point| format_rate(point.transfer_bytes_per_second))
        .unwrap_or_else(|| "—".to_owned());
    let examples = run
        .as_ref()
        .and_then(|run| run.metrics.last())
        .map(|point| format!("{:.0}/s", point.examples_per_second))
        .unwrap_or_else(|| "—".to_owned());
    let checkpoint_count = run.as_ref().map_or(0, |run| run.checkpoints.len());
    let queue_depth = run
        .as_ref()
        .and_then(|run| run.metrics.last())
        .map(|point| point.queue_depth.to_string())
        .unwrap_or_else(|| "—".to_owned());
    let source_dataset = run
        .as_ref()
        .map(|run| run.source.dataset.as_str())
        .filter(|dataset| !dataset.is_empty())
        .unwrap_or(&dataset);
    let checkpoint_table = run
        .as_ref()
        .map(|run| run.source.checkpoint_table.as_str())
        .filter(|table| !table.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{dataset}.training_checkpoints"));
    let json_fallback = run
        .as_ref()
        .is_some_and(|run| run.logs.iter().any(|line| line.contains("JSON vectors")));

    rsx! {
        section { class: "training-data-path",
            header { class: "training-panel-heading",
                div {
                    h2 { "Spiral → PyTorch" }
                    p { "One columnar path, supervised by Rust." }
                }
                span { class: "training-path-rate", "{transfer}" }
            }
            ol { class: "training-path-stages",
                li {
                    span { class: "training-path-index", "1" }
                    div { strong { "Spiral query" } small { "{source_dataset} · source frozen when the run starts" } }
                    em { "Rust ADBC" }
                }
                li {
                    span { class: "training-path-index", "2" }
                    div { strong { "Bounded Arrow IPC queue" } small { if json_fallback { "Rust normalized JSON vectors to float32" } else { "Typed float buffers use the direct tensor path" } } }
                    em { "depth {queue_depth}" }
                }
                li {
                    span { class: "training-path-index", "3" }
                    div { strong { "PyTorch compute worker" } small { "Arrow → tensor · model · optimizer only" } }
                    em { "{examples}" }
                }
                li {
                    span { class: "training-path-index", "4" }
                    div { strong { "Rust checkpoint publish" } small { "{checkpoint_table}" } }
                    em { "{checkpoint_count} durable" }
                }
            }
            div { class: "training-path-contract",
                strong { "Rust owns the run" }
                span { "Spiral access, cancellation, checkpoint ingest, durable history, and browser events stay in the Studio backend." }
            }
        }
    }
}

#[component]
fn TrainingResources(run: Option<TrainingRun>) -> Element {
    let total_steps = run.as_ref().map_or(1, |run| run.config.steps);
    let metrics = run
        .as_ref()
        .map(|run| run.metrics.as_slice())
        .unwrap_or(&[]);
    let last = metrics.last();
    let gpu = last
        .and_then(|point| point.gpu_utilization_pct)
        .map(|value| format!("{value:.0}%"))
        .unwrap_or_else(|| "—".to_owned());
    let memory = last
        .and_then(|point| point.gpu_memory_bytes)
        .map(format_bytes)
        .unwrap_or_else(|| "—".to_owned());
    let cpu = last
        .map(|point| format!("{:.0}%", point.cpu_utilization_pct))
        .unwrap_or_else(|| "—".to_owned());
    let transfer = last
        .map(|point| format_rate(point.transfer_bytes_per_second))
        .unwrap_or_else(|| "—".to_owned());

    rsx! {
        section { class: "training-resources",
            header { class: "training-resource-header",
                div {
                    h2 { "Machine and input pipeline" }
                    p { "Aligned to the same optimizer-step axis as loss." }
                }
                if let Some(point) = last {
                    span { "{format_elapsed(point.elapsed_seconds)} elapsed" }
                }
            }
            div { class: "training-resource-grid",
                ResourceLane { label: "GPU utilization", value: gpu, points: resource_points(metrics, total_steps, ResourceSeries::Gpu), class: "resource-gpu" }
                ResourceLane { label: "GPU memory", value: memory, points: resource_points(metrics, total_steps, ResourceSeries::Memory), class: "resource-memory" }
                ResourceLane { label: "CPU utilization", value: cpu, points: resource_points(metrics, total_steps, ResourceSeries::Cpu), class: "resource-cpu" }
                ResourceLane { label: "Spiral transfer", value: transfer, points: resource_points(metrics, total_steps, ResourceSeries::Transfer), class: "resource-transfer" }
            }
        }
    }
}

#[component]
fn ResourceLane(
    label: &'static str,
    value: String,
    points: String,
    class: &'static str,
) -> Element {
    rsx! {
        div { class: "training-resource-lane {class}",
            div { span { "{label}" } strong { "{value}" } }
            svg { view_box: "0 0 1000 80", preserve_aspect_ratio: "none", "aria-hidden": "true",
                line { x1: "0", x2: "1000", y1: "68", y2: "68", class: "training-resource-baseline" }
                if !points.is_empty() {
                    polyline { points, class: "training-resource-line" }
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ResourceSeries {
    Gpu,
    Memory,
    Cpu,
    Transfer,
}

pub(crate) fn default_training_sql(dataset: &str) -> String {
    format!(
        "SELECT episode_id, stream_id, sample_index,\n       state_position_json AS state,\n       action_position_json AS action\nFROM {dataset}.joint_samples\nWHERE action_position_json IS NOT NULL\nORDER BY episode_id, stream_id, sample_index"
    )
}

fn phase_class(phase: TrainingRunPhase) -> &'static str {
    match phase {
        TrainingRunPhase::Starting | TrainingRunPhase::Running | TrainingRunPhase::Stopping => {
            "training-phase training-phase-active"
        }
        TrainingRunPhase::Completed => "training-phase training-phase-complete",
        TrainingRunPhase::Stopped => "training-phase",
        TrainingRunPhase::Failed => "training-phase training-phase-failed",
    }
}

fn loss_points(run: &TrainingRun) -> (String, String, String, String) {
    let values = run
        .metrics
        .iter()
        .flat_map(|point| [Some(point.train_loss), point.validation_loss])
        .flatten()
        .filter(|value| value.is_finite() && *value > 0.0)
        .collect::<Vec<_>>();
    if values.is_empty() {
        return (String::new(), String::new(), "—".to_owned(), "—".to_owned());
    }
    let log_min = values
        .iter()
        .copied()
        .map(f64::ln)
        .fold(f64::INFINITY, f64::min);
    let log_max = values
        .iter()
        .copied()
        .map(f64::ln)
        .fold(f64::NEG_INFINITY, f64::max);
    let padding = ((log_max - log_min) * 0.08).max(0.08);
    let floor = log_min - padding;
    let ceiling = log_max + padding;
    let train = metric_points(&run.metrics, run.config.steps, floor, ceiling, |point| {
        Some(point.train_loss)
    });
    let validation = metric_points(&run.metrics, run.config.steps, floor, ceiling, |point| {
        point.validation_loss
    });
    (
        train,
        validation,
        format_loss(ceiling.exp()),
        format_loss(floor.exp()),
    )
}

fn metric_points(
    metrics: &[TrainingMetricPoint],
    total_steps: u64,
    floor: f64,
    ceiling: f64,
    value: impl Fn(&TrainingMetricPoint) -> Option<f64>,
) -> String {
    let span = (ceiling - floor).max(f64::EPSILON);
    metrics
        .iter()
        .filter_map(|point| {
            let reading = value(point)?;
            (reading.is_finite() && reading > 0.0).then(|| {
                let x = point.step as f64 / total_steps.max(1) as f64 * CHART_WIDTH;
                let normalized = ((reading.ln() - floor) / span).clamp(0.0, 1.0);
                let y = CHART_BOTTOM - normalized * (CHART_BOTTOM - CHART_TOP);
                format!("{x:.2},{y:.2}")
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn resource_points(
    metrics: &[TrainingMetricPoint],
    total_steps: u64,
    series: ResourceSeries,
) -> String {
    let readings = metrics
        .iter()
        .filter_map(|point| match series {
            ResourceSeries::Gpu => point.gpu_utilization_pct,
            ResourceSeries::Memory => point.gpu_memory_bytes.map(|value| value as f64),
            ResourceSeries::Cpu => Some(point.cpu_utilization_pct),
            ResourceSeries::Transfer => Some(point.transfer_bytes_per_second),
        })
        .filter(|value| value.is_finite() && *value >= 0.0)
        .collect::<Vec<_>>();
    let maximum = match series {
        ResourceSeries::Gpu | ResourceSeries::Cpu => 100.0,
        ResourceSeries::Memory | ResourceSeries::Transfer => {
            readings.iter().copied().fold(0.0, f64::max).max(1.0)
        }
    };
    metrics
        .iter()
        .filter_map(|point| {
            let value = match series {
                ResourceSeries::Gpu => point.gpu_utilization_pct?,
                ResourceSeries::Memory => point.gpu_memory_bytes? as f64,
                ResourceSeries::Cpu => point.cpu_utilization_pct,
                ResourceSeries::Transfer => point.transfer_bytes_per_second,
            };
            let x = point.step as f64 / total_steps.max(1) as f64 * CHART_WIDTH;
            let y = 68.0 - (value / maximum).clamp(0.0, 1.0) * 56.0;
            Some(format!("{x:.2},{y:.2}"))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn step_percent(step: u64, total_steps: u64) -> f64 {
    (step as f64 / total_steps.max(1) as f64 * 100.0).clamp(0.0, 100.0)
}

fn metric_event_label(point: &TrainingMetricPoint) -> String {
    format!(
        "{} at step {}",
        point.event.as_deref().unwrap_or("Event"),
        point.step
    )
}

fn format_loss(value: f64) -> String {
    if value >= 0.01 {
        format!("{value:.4}")
    } else {
        format!("{value:.2e}")
    }
}

fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.0} MiB", bytes as f64 / MIB)
    }
}

fn format_rate(bytes_per_second: f64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes_per_second >= GIB {
        format!("{:.2} GiB/s", bytes_per_second / GIB)
    } else {
        format!("{:.1} MiB/s", bytes_per_second / MIB)
    }
}

fn format_elapsed(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let hours = total / 3_600;
    let minutes = total % 3_600 / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_query_uses_only_spiral_joint_samples() {
        let sql = default_training_sql("robotics");
        assert!(sql.contains("FROM robotics.joint_samples"));
        assert!(sql.contains("action_position_json IS NOT NULL"));
    }

    #[test]
    fn resources_use_the_optimizer_step_axis() {
        let metrics = vec![TrainingMetricPoint {
            step: 50,
            cpu_utilization_pct: 25.0,
            ..TrainingMetricPoint::default()
        }];
        assert!(resource_points(&metrics, 100, ResourceSeries::Cpu).starts_with("500.00,"));
        assert_eq!(step_percent(50, 100), 50.0);
    }
}
