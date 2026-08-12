//! Curve-first Training screen workspace.
//!
//! This surface is intentionally a browser over Rust-owned state. It starts and
//! stops runs through the backend, then renders only metrics received from the
//! Studio event stream; no training lifecycle is inferred in the browser.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use spiral_robotics_core::api::{
    TrainingMetricPoint, TrainingReadiness, TrainingRun, TrainingRunPhase,
};
use spiral_robotics_core::spiral::{self, Table};
use spiral_robotics_panels::{LayoutNode, Panel, PanelLayout, PanelWorkspace, SplitAxis};

use crate::frontend::app::notifications::MessageSource;
use crate::frontend::app::state::studio;
use crate::frontend::screens::training::state::training_controller;

const CHART_WIDTH: f64 = 1_000.0;
const CHART_TOP: f64 = 18.0;
const CHART_BOTTOM: f64 = 242.0;

#[component]
pub(crate) fn TrainingWorkspace(dataset: String) -> Element {
    let studio = studio();
    let dataset_id = studio.selected_dataset.read().clone();
    let controller = training_controller();
    let snapshot = studio.training.read().clone();
    let active_run = snapshot.active_run().cloned();
    let viewed_run = controller
        .selected_run()
        .as_ref()
        .and_then(|id| snapshot.runs.iter().find(|run| &run.id == id))
        .or(active_run.as_ref())
        .cloned();
    let readiness_notice = (!snapshot.readiness.ready
        && !snapshot.readiness.summary.starts_with("Checking "))
    .then(|| training_readiness_guidance(&snapshot.readiness).0);
    use_effect(use_reactive((&readiness_notice,), move |(notice,)| {
        let Some(notice) = notice else { return };
        if !studio
            .notifications
            .read()
            .contains(MessageSource::Training, &notice)
        {
            studio.report_warning_from(MessageSource::Training, notice);
        }
    }));

    let workbench = Panel::new(
        "training-workbench",
        "Training",
        "training-workbench",
        rsx! {
            TrainingRunBuilder {
                namespace: dataset.clone(),
                dataset_id: dataset_id.clone(),
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
        rsx! { TrainingDataPath { namespace: dataset.clone(), dataset_id: dataset_id.clone(), run: viewed_run.clone() } },
    )
    .with_class("training-data-editor");
    let throughput = Panel::new(
        "training-resources",
        "Throughput",
        "training-resources",
        rsx! { TrainingThroughput { run: viewed_run } },
    )
    .with_class("training-resource-editor");
    let panels = vec![workbench, learning, data_path, throughput];

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
            storage_key: Some("spiral_robotics.panel-layout.v1.training".to_owned()),
            on_layout_change: move |_layout: PanelLayout| {},
            on_panel_activate: move |_id| {},
            on_resize: move |_| {},
        }
    }
}

#[component]
fn TrainingRunBuilder(
    namespace: String,
    dataset_id: String,
    readiness: TrainingReadiness,
    active_run: Option<TrainingRun>,
    viewed_run: Option<TrainingRun>,
    history: Vec<TrainingRun>,
) -> Element {
    let studio = studio();
    let mut selected_dataset = studio.selected_dataset;
    let controller = training_controller();
    let request = controller.request();
    let inspected_request = viewed_run
        .as_ref()
        .map(|run| run.config.clone())
        .unwrap_or_else(|| request.clone());
    let inspecting_run = viewed_run.as_ref().is_some_and(|run| !run.phase.active());
    let dataset_ids = studio
        .episodes()
        .into_iter()
        .map(|episode| episode.dataset_id)
        .chain(std::iter::once(dataset_id.clone()))
        .collect::<BTreeSet<_>>();
    let selected_run = controller.selected_run();
    let starting = controller.starting();
    let stopping = controller.stopping();
    let dataset_valid = spiral::is_dataset_id(dataset_id.trim());
    let can_start = readiness.ready && active_run.is_none() && !starting && dataset_valid;
    let generated_dataset_query =
        inspected_request.sql == default_training_sql(&namespace, &inspected_request.dataset_id);
    let (local_episode_count, local_frame_count) = studio
        .episodes()
        .into_iter()
        .filter(|episode| {
            episode.dataset_id == inspected_request.dataset_id
                && episode.usable()
                && !episode.discarded
        })
        .fold((0_u64, 0_u64), |(episodes, frames), episode| {
            (episodes + 1, frames.saturating_add(episode.frames))
        });
    let active_id = active_run.as_ref().map(|run| run.id.clone());
    let progress = active_run.as_ref().map(|run| {
        if run.config.steps == 0 {
            0.0
        } else {
            run.current_step as f64 / run.config.steps as f64 * 100.0
        }
    });
    let (readiness_summary, readiness_recovery) = training_readiness_guidance(&readiness);

    rsx! {
        div { class: "training-workbench studio-workbench-scroll",
            if !readiness.ready {
                div { class: "training-inline-state training-inline-state-caution",
                    div { class: "training-inline-copy",
                        strong { "Training is not ready" }
                        span { "{readiness_summary}" }
                        p { "{readiness_recovery}" }
                        details { class: "technical-details",
                            summary { "Technical details" }
                            code { "{readiness.summary}" }
                        }
                    }
                }
            }

            if let Some(run) = active_run.clone() {
                section { class: "training-active-run",
                    div { class: "training-active-heading",
                        div {
                            strong { "{run.config.run_name}" }
                            span { class: phase_class(run.phase), "{run.phase.label()}" }
                            small { "Dataset · {run.config.dataset_id}" }
                        }
                        span { class: "training-step-readout", "{run.current_step} / {run.config.steps}" }
                    }
                    div {
                        class: "training-progress-track",
                        role: "progressbar",
                        "aria-label": "Training progress",
                        "aria-valuemin": "0",
                        "aria-valuemax": "{run.config.steps}",
                        "aria-valuenow": "{run.current_step}",
                        "aria-valuetext": "Step {run.current_step} of {run.config.steps}",
                        div { class: "training-progress-fill", style: "width: {progress.unwrap_or_default().clamp(0.0, 100.0):.2}%" }
                    }
                    if run.phase != TrainingRunPhase::Running {
                        if let Some(message) = run.message {
                        p { "{message}" }
                        }
                    }
                    button {
                        r#type: "button",
                        class: "training-stop-button",
                        title: "Stop after the current optimizer step and publish a checkpoint",
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
                        div { class: "training-run-result-heading",
                            h3 { "{run.config.run_name}" }
                            span { class: phase_class(run.phase), "{run.phase.label()}" }
                        }
                        p { {run.message.clone().unwrap_or_else(|| "The run ended without a status message.".to_owned())} }
                        button {
                            r#type: "button",
                            class: "link min-h-6",
                            onclick: move |_| controller.clear_selected_run(),
                            "Back to new run"
                        }
                        if !run.logs.is_empty() {
                            details {
                                summary { "Logs · {run.logs.len()}" }
                                pre {
                                    for line in run.logs.iter().rev().take(12).rev() {
                                        code { "{line}\n" }
                                    }
                                }
                            }
                        }
                    }
            }

            section { class: "training-input-summary",
                header {
                    h3 { "Training data" }
                    span { if inspecting_run { "Run snapshot" } else { "New run draft" } }
                }
                dl {
                    div { class: "training-dataset-definition",
                        dt { "Dataset" }
                        dd {
                            if inspecting_run || active_run.is_some() {
                                "{inspected_request.dataset_id}"
                            } else {
                                input {
                                    list: "training-dataset-options",
                                    value: dataset_id.clone(),
                                    maxlength: "96",
                                    pattern: "[A-Za-z0-9_.-]+",
                                    "aria-label": "Training dataset",
                                    oninput: move |event| selected_dataset.set(event.value()),
                                }
                            }
                        }
                    }
                    div { dt { "Namespace" } dd { "{namespace}" } }
                    div { dt { "SQL scope" }
                        dd { if generated_dataset_query { "Filtered by episodes.dataset_id" } else { "Custom SQL · review below" } }
                    }
                    div { dt { "Local preview" } dd { "{local_episode_count} kept · {local_frame_count} frames" } }
                    div { dt { "Cameras" } dd { "{inspected_request.act.camera_streams.len()} required" } }
                }
                datalist { id: "training-dataset-options",
                    for id in dataset_ids { option { value: "{id}" } }
                }
                details {
                    summary { "Exact SQL and column mapping" }
                    pre { code { "{inspected_request.sql}" } }
                    p {
                        "State · {inspected_request.state_column} · Action · {inspected_request.action_column}"
                    }
                }
            }

            if !inspecting_run {
            section { class: "training-form-section",
                header { class: "training-form-heading",
                    h3 { "New run" }
                    span { "Dataset · {dataset_id}" }
                }
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

            details { class: "training-query-section",
                summary {
                    span { "New run data query" }
                    small { "{request.dataset_id}" }
                }
                div { class: "training-query-body",
                    textarea {
                        "aria-label": "Spiral SQL query",
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
                    div { class: "training-column-row",
                        label { class: "training-field training-field-wide",
                            span { "Camera streams" }
                            input {
                                value: request.act.camera_streams.join(", "),
                                placeholder: "follower.camera.gripper, follower.camera.top",
                                disabled: active_run.is_some(),
                                oninput: move |event| {
                                    let streams = event.value()
                                        .split(',')
                                        .map(str::trim)
                                        .filter(|stream| !stream.is_empty())
                                        .map(str::to_owned)
                                        .collect();
                                    controller.edit_request(move |request| request.act.camera_streams = streams);
                                },
                            }
                        }
                        label { class: "training-field",
                            span { "Image width" }
                            input {
                                r#type: "number", min: "32", max: "1024", step: "16",
                                value: "{request.act.image_width}",
                                disabled: active_run.is_some(),
                                oninput: move |event| if let Ok(value) = event.value().parse() {
                                    controller.edit_request(move |request| request.act.image_width = value);
                                },
                            }
                        }
                        label { class: "training-field",
                            span { "Image height" }
                            input {
                                r#type: "number", min: "32", max: "1024", step: "16",
                                value: "{request.act.image_height}",
                                disabled: active_run.is_some(),
                                oninput: move |event| if let Ok(value) = event.value().parse() {
                                    controller.edit_request(move |request| request.act.image_height = value);
                                },
                            }
                        }
                    }
                }
            }

            button {
                r#type: "button",
                class: "training-start-button",
                disabled: !can_start,
                title: if active_run.is_some() { "Stop the active run first".to_owned() } else if !dataset_valid { "Choose a valid dataset ID first".to_owned() } else if !readiness.ready { readiness_recovery.clone() } else { "Start training".to_owned() },
                onclick: move |_| {
                    controller.start(studio);
                },
                if starting { "Starting…" } else { "Train {request.dataset_id}" }
            }
            }

            section { class: "training-history",
                header {
                    h3 { "Run history" }
                }
                if history.is_empty() {
                    p { class: "training-history-empty", "No runs yet." }
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
                        span { class: "training-history-dataset", "{run.config.dataset_id}" }
                        span { class: phase_class(run.phase), "{run.phase.label()}" }
                        span { class: "training-history-step", "{run.current_step}" }
                    }
                }
            }
        }
    }
}

fn training_readiness_guidance(readiness: &TrainingReadiness) -> (String, String) {
    if !readiness.spiral_ready {
        return (
            "SpiralDB is not ready for training.".to_owned(),
            "Open Settings, verify the Spiral catalog, project, namespace, and ADBC driver, then test the connection again."
                .to_owned(),
        );
    }
    (readiness.summary.clone(), "Training is ready.".to_owned())
}

#[component]
fn TrainingLearning(run: Option<TrainingRun>) -> Element {
    let Some(run) = run else {
        return rsx! {
            div { class: "training-panel-empty",
                strong { "No run" }
                span { "Start training to see loss." }
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
                    p { "{run.config.policy.label()} · {run.config.device} · {run.current_step}/{run.config.steps}" }
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
                            "No metrics."
                        } else if run.phase.active() {
                            "Waiting for step 1…"
                        } else {
                            "No metrics."
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
                    small { "Val" }
                    small { "Checkpoint" }
                    small { "Stall" }
                }
            }
        }
    }
}

#[component]
fn TrainingDataPath(namespace: String, dataset_id: String, run: Option<TrainingRun>) -> Element {
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
    let frames_per_batch = run
        .as_ref()
        .map(|run| run.config.batch_size * run.config.act.camera_streams.len())
        .map(|frames| format!("{frames}/batch"))
        .unwrap_or_else(|| "—".to_owned());
    let source_namespace = run
        .as_ref()
        .map(|run| run.source.dataset.as_str())
        .filter(|dataset| !dataset.is_empty())
        .unwrap_or(&namespace);
    let source_dataset = run
        .as_ref()
        .map(|run| run.source.dataset_id.as_str())
        .filter(|dataset| !dataset.is_empty())
        .unwrap_or(&dataset_id);
    let checkpoint_table = run
        .as_ref()
        .map(|run| run.source.checkpoint_table.as_str())
        .filter(|table| !table.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| spiral::relation(&namespace, Table::TrainingCheckpoints));
    rsx! {
        section { class: "training-data-path",
            ol { class: "training-path-stages",
                li {
                    span { class: "training-path-index", "1" }
                    div { strong { "Dataset · {source_dataset}" } small { "{source_namespace}.episodes → joint_samples" } }
                    em { "{transfer}" }
                }
                li {
                    span { class: "training-path-index", "2" }
                    div { strong { "Arrow tensors" } small { "decode · resize · normalize" } }
                    em { "{frames_per_batch}" }
                }
                li {
                    span { class: "training-path-index", "3" }
                    div { strong { "Burn ACT" } small { "native optimizer" } }
                    em { "{examples}" }
                }
                li {
                    span { class: "training-path-index", "4" }
                    div { strong { "Checkpoints" } small { "{checkpoint_table}" } }
                    em { "{checkpoint_count}" }
                }
            }
        }
    }
}

#[component]
fn TrainingThroughput(run: Option<TrainingRun>) -> Element {
    let total_steps = run.as_ref().map_or(1, |run| run.config.steps);
    let metrics = run
        .as_ref()
        .map(|run| run.metrics.as_slice())
        .unwrap_or(&[]);
    let last = metrics.last();
    let throughput = last
        .map(|point| format!("{:.1} examples/s", point.examples_per_second))
        .unwrap_or_else(|| "—".to_owned());
    let transfer = last
        .map(|point| format_rate(point.transfer_bytes_per_second))
        .unwrap_or_else(|| "—".to_owned());

    rsx! {
        section { class: "training-resources",
            div { class: "training-resource-grid",
                ResourceLane { label: "Optimizer throughput", value: throughput, points: resource_points(metrics, total_steps, ResourceSeries::Throughput), class: "resource-throughput", description: "Examples completed per second over the latest measured optimizer interval." }
                ResourceLane { label: "Spiral tensor rate", value: transfer, points: resource_points(metrics, total_steps, ResourceSeries::Transfer), class: "resource-transfer", description: "Decoded image-tensor bytes returned by Spiral divided by total sample-preparation time." }
            }
            div { class: "training-resource-definitions",
                small { "Optimizer throughput counts completed examples over the latest metric interval." }
                small { "Spiral tensor rate = decoded Arrow image-tensor bytes ÷ batch preparation time. It excludes Burn model compute and is not raw network speed." }
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
    description: &'static str,
) -> Element {
    rsx! {
        div { class: "training-resource-lane {class}", title: "{description}",
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
    Throughput,
    Transfer,
}

pub(crate) fn default_training_sql(namespace: &str, dataset_id: &str) -> String {
    spiral::default_training_sql_for_dataset(namespace, dataset_id)
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
        .map(|point| match series {
            ResourceSeries::Throughput => point.examples_per_second,
            ResourceSeries::Transfer => point.transfer_bytes_per_second,
        })
        .filter(|value| value.is_finite() && *value >= 0.0)
        .collect::<Vec<_>>();
    let maximum = match series {
        ResourceSeries::Throughput | ResourceSeries::Transfer => {
            readings.iter().copied().fold(0.0, f64::max).max(1.0)
        }
    };
    metrics
        .iter()
        .map(|point| {
            let value = match series {
                ResourceSeries::Throughput => point.examples_per_second,
                ResourceSeries::Transfer => point.transfer_bytes_per_second,
            };
            let x = point.step as f64 / total_steps.max(1) as f64 * CHART_WIDTH;
            let y = 68.0 - (value / maximum).clamp(0.0, 1.0) * 56.0;
            format!("{x:.2},{y:.2}")
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

fn format_rate(bytes_per_second: f64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes_per_second >= GIB {
        format!("{:.2} GiB/s", bytes_per_second / GIB)
    } else {
        format!("{:.1} MiB/s", bytes_per_second / MIB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_query_selects_complete_kept_follower_samples() {
        let sql = default_training_sql("robotics", "pick-place");
        assert!(sql.contains(&format!(
            "FROM {}",
            spiral::relation("robotics", Table::JointSamples)
        )));
        assert!(sql.contains(&spiral::relation("robotics", Table::Episodes)));
        assert!(sql.contains(&spiral::relation("robotics", Table::EpisodeStreams)));
        assert!(sql.contains(&format!("j.{}", spiral::column::STATE_VALID)));
        assert!(sql.contains(&format!("j.{}", spiral::column::ACTION_VALID)));
        assert!(sql.contains("LIKE '%follower%'"));
    }

    #[test]
    fn throughput_uses_the_optimizer_step_axis() {
        let metrics = vec![TrainingMetricPoint {
            step: 50,
            examples_per_second: 25.0,
            ..TrainingMetricPoint::default()
        }];
        assert!(resource_points(&metrics, 100, ResourceSeries::Throughput).starts_with("500.00,"));
        assert_eq!(step_percent(50, 100), 50.0);
    }
}
