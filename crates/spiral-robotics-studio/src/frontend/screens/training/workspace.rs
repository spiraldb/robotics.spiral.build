//! Training setup and run monitoring.

use std::collections::BTreeSet;

use charming::Chart;
use charming::component::{Axis, DataZoom, DataZoomType, FilterMode, Grid};
use charming::datatype::{CompositeValue, DataFrame, DataPoint};
use charming::element::{
    AreaStyle, AxisLabel, AxisLine, AxisTick, AxisType, Emphasis, LineStyle, Sampling, SplitLine,
    Symbol,
};
use charming::series::Line;
use dioxus::prelude::*;
use spiral_robotics_contracts::api::{
    TrainingMetricPoint, TrainingReadiness, TrainingRun, TrainingRunPhase,
};
use spiral_robotics_contracts::spiral;
use spiral_robotics_panels::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};

use crate::frontend::app::state::studio;
use crate::frontend::components::charts::{
    COLOR_CPU, COLOR_GPU, COLOR_INK_FAINT, COLOR_LINE, COLOR_MEMORY, COLOR_SELECTED, COLOR_SURFACE,
    COLOR_THROUGHPUT, COLOR_TRAIN, COLOR_TRANSFER, COLOR_VALIDATION, CharmingChart, MONO_FONT,
    TooltipRateUnit, downsample_extrema, studio_bytes_tooltip, studio_numeric_tooltip,
    studio_percent_tooltip, studio_rate_tooltip,
};
use crate::frontend::screens::training::state::training_controller;

const EMPTY_RUN_PANEL_ID: &str = "training-run-empty";
const TRAINING_CHART_POINT_BUDGET: usize = 1_200;

#[component]
pub(crate) fn TrainingWorkspace() -> Element {
    let studio = studio();
    let dataset_id = studio.selected_dataset.read().clone();
    let controller = training_controller();
    let snapshot = studio.training.read().clone();
    let active_run = snapshot.active_run().cloned();
    let active_run_id = active_run.as_ref().map(|run| run.id.clone());
    use_effect(use_reactive((&active_run_id,), move |(run_id,)| {
        if let Some(run_id) = run_id {
            controller.open_run(run_id);
        }
    }));

    let control = Panel::new(
        "training-control",
        "Setup",
        "training-control",
        rsx! {
            TrainingRunControl {
                dataset_id: dataset_id.clone(),
                readiness: snapshot.readiness.clone(),
                active_run: active_run.clone(),
            }
        },
    )
    .with_class("training-control-editor");
    let history = Panel::new(
        "training-history",
        "Runs",
        "training-history",
        rsx! { TrainingHistory { history: snapshot.runs.clone() } },
    )
    .with_class("training-history-editor");

    let opened_ids = controller.opened_runs();
    let opened_runs = opened_ids
        .iter()
        .filter_map(|id| snapshot.runs.iter().find(|run| &run.id == id).cloned())
        .collect::<Vec<_>>();
    let registered_runs = opened_runs
        .iter()
        .map(|run| (run_panel_id(&run.id), run.id.clone()))
        .collect::<Vec<_>>();
    let mut panels = vec![control, history];
    let right_panel_ids = if opened_runs.is_empty() {
        panels.push(
            Panel::new(
                EMPTY_RUN_PANEL_ID,
                "Run",
                "training-runs",
                rsx! { TrainingRunEmpty {} },
            )
            .with_class("training-run-editor"),
        );
        vec![EMPTY_RUN_PANEL_ID.to_owned()]
    } else {
        let ids = opened_runs
            .iter()
            .map(|run| run_panel_id(&run.id))
            .collect::<Vec<_>>();
        for run in opened_runs {
            let panel_id = run_panel_id(&run.id);
            panels.push(
                Panel::new(
                    panel_id,
                    run.config.run_name.clone(),
                    "training-runs",
                    rsx! {
                        TrainingRunDetail {
                            run,
                        }
                    },
                )
                .with_class("training-run-editor")
                .with_closable(true),
            );
        }
        ids
    };

    let selected_panel = controller
        .selected_run()
        .filter(|id| snapshot.runs.iter().any(|run| &run.id == id))
        .map(|id| PanelId::from(run_panel_id(&id)));
    let closeable_runs = registered_runs.clone();
    let left = LayoutNode::split(
        "training-left-rail",
        SplitAxis::Vertical,
        0.68,
        LayoutNode::tile("training-control", ["training-control".to_owned()]),
        LayoutNode::tile("training-history", ["training-history".to_owned()]),
    );
    let layout = PanelLayout::new(LayoutNode::split(
        "training-columns",
        SplitAxis::Horizontal,
        0.3,
        left,
        LayoutNode::tile("training-runs", right_panel_ids),
    ));

    rsx! {
        PanelWorkspace {
            panels,
            initial_layout: layout,
            storage_key: Some("spiral_robotics.panel-layout.v2.training".to_owned()),
            active_panel: selected_panel,
            on_layout_change: move |_layout: PanelLayout| {},
            on_panel_activate: move |panel_id: PanelId| {
                if let Some((_, run_id)) = registered_runs
                    .iter()
                    .find(|(registered, _)| registered == panel_id.as_str())
                {
                    controller.select_run(run_id.clone());
                }
            },
            on_panel_close: move |panel_id: PanelId| {
                if let Some((_, run_id)) = closeable_runs
                    .iter()
                    .find(|(registered, _)| registered == panel_id.as_str())
                {
                    controller.close_run(run_id.clone());
                }
            },
            on_resize: move |_| {},
        }
    }
}

#[component]
fn TrainingRunControl(
    dataset_id: String,
    readiness: TrainingReadiness,
    active_run: Option<TrainingRun>,
) -> Element {
    let studio = studio();
    let mut selected_dataset = studio.selected_dataset;
    let controller = training_controller();
    let request = controller.request();
    let dataset_ids = studio
        .episodes()
        .into_iter()
        .map(|episode| episode.dataset_id)
        .chain(std::iter::once(dataset_id.clone()))
        .collect::<BTreeSet<_>>();
    let starting = controller.starting();
    let stopping = controller.stopping();
    let dataset_valid = spiral::is_dataset_id(dataset_id.trim());
    let can_start = readiness.ready && active_run.is_none() && !starting && dataset_valid;
    let active_id = active_run.as_ref().map(|run| run.id.clone());
    let progress = active_run.as_ref().map(|run| {
        if run.config.steps == 0 {
            0.0
        } else {
            run.current_step as f64 / run.config.steps as f64 * 100.0
        }
    });
    let (readiness_summary, readiness_recovery) = training_readiness_guidance(&readiness);
    let camera_summary = if request.act.camera_streams.is_empty() {
        "All available".to_owned()
    } else {
        format!(
            "{} · {}",
            request.act.camera_streams.len(),
            request.act.camera_streams.join(", ")
        )
    };
    let validation_percent = request.validation_split * 100.0;

    rsx! {
        div { class: "training-control",
            div { class: "training-control-scroll",
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
                        title: "Finish the current step, save a checkpoint, and stop",
                        disabled: run.phase == TrainingRunPhase::Stopping || stopping.is_some(),
                        onclick: move |_| {
                            let id = active_id.clone().unwrap_or_default();
                            controller.stop(studio, id);
                        },
                        if run.phase == TrainingRunPhase::Stopping || stopping.is_some() { "Stopping…" } else { "Stop training" }
                    }
                }
            }

            section { class: "training-form-section training-source-section",
                header { class: "training-form-heading",
                    h3 { "Training data" }
                }
                label { class: "training-field training-field-wide",
                    span { "Dataset" }
                    select {
                        value: dataset_id.clone(),
                        disabled: active_run.is_some(),
                        "aria-label": "Training dataset",
                        onchange: move |event| selected_dataset.set(event.value()),
                        for id in dataset_ids.iter() {
                            option { value: "{id}", "{id}" }
                        }
                    }
                }
                dl { class: "training-source-meta",
                    div { dt { "Cameras" } dd { "{camera_summary}" } }
                }
            }

            section { class: "training-form-section",
                header { class: "training-form-heading",
                    h3 { "Run" }
                }
                div { class: "training-form-grid training-primary-grid",
                    label { class: "training-field",
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
                }
            }

            details { class: "training-query-section",
                summary { "Advanced settings" }
                div { class: "training-query-body",
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
                            option { value: "mps", "Apple MPS" }
                            option { value: "cpu", "CPU" }
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
                        span { "Validation holdout (%)" }
                        input {
                            r#type: "number", min: "0", max: "50", step: "1",
                            title: "Whole episodes reserved for validation and excluded from optimizer updates",
                            value: "{validation_percent}",
                            disabled: active_run.is_some(),
                            oninput: move |event| if let Ok(value) = event.value().parse::<f64>() {
                                controller.edit_request(move |request| request.validation_split = value / 100.0);
                            },
                        }
                    }
                    label { class: "training-field",
                        span { "Action horizon" }
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
                        span { "Checkpoint interval" }
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
                    div { class: "training-advanced-label", "Data query" }
                    textarea {
                        "aria-label": "Training data query",
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
                                placeholder: "All available",
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
                            span { "Image width (0 = source)" }
                            input {
                                r#type: "number", min: "0", max: "1024", step: "1",
                                value: "{request.act.image_width}",
                                disabled: active_run.is_some(),
                                oninput: move |event| if let Ok(value) = event.value().parse() {
                                    controller.edit_request(move |request| request.act.image_width = value);
                                },
                            }
                        }
                        label { class: "training-field",
                            span { "Image height (0 = source)" }
                            input {
                                r#type: "number", min: "0", max: "1024", step: "1",
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
            }

            button {
                r#type: "button",
                class: "training-start-button",
                disabled: !can_start,
                title: if active_run.is_some() { "Stop the active run first".to_owned() } else if !dataset_valid { "Choose a valid dataset ID first".to_owned() } else if !readiness.ready { readiness_recovery.clone() } else { "Start training".to_owned() },
                onclick: move |_| {
                    controller.start(studio);
                },
                if starting { "Starting…" } else { "Run training" }
            }
        }
    }
}

#[component]
fn TrainingHistory(history: Vec<TrainingRun>) -> Element {
    let controller = training_controller();
    let selected_run = controller.selected_run();
    rsx! {
        section { class: "training-history-panel studio-workbench-scroll",
            if history.is_empty() {
                div { class: "training-history-empty",
                    strong { "No runs yet" }
                    span { "Start training to create one." }
                }
            }
            for run in history {
                button {
                    r#type: "button",
                    class: if selected_run.as_deref() == Some(run.id.as_str()) { "training-history-row training-history-row-selected" } else { "training-history-row" },
                    "aria-pressed": selected_run.as_deref() == Some(run.id.as_str()),
                    title: "Open {run.config.run_name}",
                    onclick: {
                        let id = run.id.clone();
                        move |_| controller.open_run(id.clone())
                    },
                    span { class: "training-history-name", "{run.config.run_name}" }
                    span { class: "training-history-dataset", "{run.config.dataset_id}" }
                    span { class: phase_class(run.phase), "{run.phase.label()}" }
                    span { class: "training-history-step", "{run.current_step} / {run.config.steps}" }
                }
            }
        }
    }
}

#[component]
fn TrainingRunEmpty() -> Element {
    rsx! {
        section { class: "training-run-empty",
            strong { "Start training or select a run" }
        }
    }
}

#[component]
fn TrainingRunDetail(run: TrainingRun) -> Element {
    let progress = if run.config.steps == 0 {
        0.0
    } else {
        run.current_step as f64 / run.config.steps as f64 * 100.0
    };
    let message = run.message.clone();
    let logs = run.logs.clone();
    let message_class = if run.phase == TrainingRunPhase::Failed {
        "training-run-message training-run-message-danger"
    } else {
        "training-run-message"
    };
    rsx! {
        article { class: "training-run-detail",
            header { class: "training-run-overview",
                div { class: "training-run-identity",
                    div {
                        h2 { "{run.config.run_name}" }
                        span { class: phase_class(run.phase), "{run.phase.label()}" }
                    }
                    p { "Dataset · {run.config.dataset_id} · {run.config.policy.label()} · {run.config.device}" }
                }
                dl { class: "training-run-facts",
                    div { dt { "Step" } dd { "{run.current_step} / {run.config.steps}" } }
                    div { dt { "Progress" } dd { "{progress.clamp(0.0, 100.0):.1}%" } }
                    div { dt { "Checkpoints" } dd { "{run.checkpoints.len()}" } }
                }
            }
            if let Some(message) = message {
                div { class: "{message_class}", "{message}" }
            }
            div { class: "training-run-evidence",
                section { class: "training-run-region training-run-learning-region",
                    header { class: "training-subpanel-heading",
                        h3 { "Loss" }
                    }
                    div { class: "training-subpanel-body",
                        TrainingLearning { run: run.clone() }
                    }
                }
                section { class: "training-run-region training-run-data-region",
                    header { class: "training-subpanel-heading",
                        h3 { "Inputs" }
                    }
                    div { class: "training-subpanel-body",
                        TrainingInputs { run: run.clone() }
                    }
                }
                section { class: "training-run-region training-run-throughput-region",
                    header { class: "training-subpanel-heading",
                        h3 { "Performance" }
                    }
                    div { class: "training-subpanel-body",
                        TrainingPerformance { run: run.clone() }
                    }
                }
            }
            if !logs.is_empty() {
                details { class: "training-run-logs",
                    summary { "Log · {logs.len()} lines" }
                    pre {
                        for line in logs.iter().rev().take(20).rev() {
                            code { "{line}\n" }
                        }
                    }
                }
            }
        }
    }
}

fn run_panel_id(run_id: &str) -> String {
    format!("training-run-{run_id}")
}

fn training_readiness_guidance(readiness: &TrainingReadiness) -> (String, String) {
    if !readiness.spiral_ready {
        return (
            "SpiralDB is unavailable.".to_owned(),
            "Open Settings and test the SpiralDB connection.".to_owned(),
        );
    }
    (readiness.summary.clone(), "Training is ready.".to_owned())
}

#[component]
fn TrainingLearning(run: TrainingRun) -> Element {
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
    let chart_id = chart_dom_id("training-loss", &run.id);
    let loss_chart = training_loss_chart(&run);

    rsx! {
        section { class: "training-learning",
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
                CharmingChart {
                    id: chart_id,
                    chart: loss_chart,
                    class: "training-loss-chart".to_owned(),
                    aria_label: "Training and validation loss by optimizer step".to_owned(),
                }
            }
            div { class: "training-event-strip",
                div { class: "training-event-track",
                    i { class: "training-event-axis" }
                    for checkpoint in &run.checkpoints {
                        span {
                            class: "training-event-tick training-event-checkpoint",
                            style: "left: {step_percent(checkpoint.step, run.config.steps):.3}%",
                            title: "Checkpoint saved at step {checkpoint.step}",
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
fn TrainingInputs(run: TrainingRun) -> Element {
    let cameras = if run.config.act.camera_streams.is_empty() {
        "All available".to_owned()
    } else {
        run.config.act.camera_streams.join(", ")
    };
    let image_size = match (run.config.act.image_width, run.config.act.image_height) {
        (0, 0) => "Source size".to_owned(),
        (0, height) => format!("Source width × {height}"),
        (width, 0) => format!("{width} × source height"),
        (width, height) => format!("{width} × {height}"),
    };
    let validation = if run.config.validation_split == 0.0 {
        "Off".to_owned()
    } else {
        format!("{:.0}% · episode", run.config.validation_split * 100.0)
    };
    rsx! {
        section { class: "training-inputs",
            dl { class: "training-input-facts",
                div { class: "training-input-wide",
                    dt { "Cameras" }
                    dd { "{cameras}" }
                }
                div { dt { "Batch" } dd { "{run.config.batch_size} samples" } }
                div { dt { "Validation" } dd { "{validation}" } }
                div { dt { "Action horizon" } dd { "{run.config.chunk_size} steps" } }
                div { dt { "Image size" } dd { "{image_size}" } }
                div { dt { "Checkpoint interval" } dd { "{run.config.checkpoint_every} steps" } }
            }
        }
    }
}

#[component]
fn TrainingPerformance(run: TrainingRun) -> Element {
    let total_steps = run.config.steps;
    let metrics = run.metrics.as_slice();
    let last = metrics.last();
    let throughput = last
        .map(|point| format!("{:.1} samples/s", point.examples_per_second))
        .unwrap_or_else(|| "—".to_owned());
    let transfer = last
        .map(|point| format_rate(point.transfer_bytes_per_second))
        .unwrap_or_else(|| "—".to_owned());
    let cpu = last
        .and_then(|point| point.cpu_percent)
        .map(|value| format!("{value:.1}%"))
        .unwrap_or_else(|| "—".to_owned());
    let gpu = last
        .and_then(|point| point.gpu_percent)
        .map(|value| format!("{value:.1}%"))
        .unwrap_or_else(|| "—".to_owned());
    let memory = last
        .and_then(|point| point.memory_bytes)
        .map(format_bytes)
        .unwrap_or_else(|| "—".to_owned());
    let throughput_chart =
        training_resource_chart(metrics, total_steps, ResourceSeries::Throughput);
    let transfer_chart = training_resource_chart(metrics, total_steps, ResourceSeries::Transfer);
    let cpu_chart = training_resource_chart(metrics, total_steps, ResourceSeries::Cpu);
    let gpu_chart = training_resource_chart(metrics, total_steps, ResourceSeries::Gpu);
    let memory_chart = training_resource_chart(metrics, total_steps, ResourceSeries::Memory);

    rsx! {
        section { class: "training-resources",
            div { class: "training-resource-grid",
                ResourceLane { id: chart_dom_id("training-rate", &run.id), label: "Training rate", value: throughput, chart: throughput_chart, class: "resource-throughput", description: "Samples completed per second." }
                ResourceLane { id: chart_dom_id("input-rate", &run.id), label: "Input rate", value: transfer, chart: transfer_chart, class: "resource-transfer", description: "Prepared camera data per second." }
                ResourceLane { id: chart_dom_id("cpu-utilization", &run.id), label: "CPU", value: cpu, chart: cpu_chart, class: "resource-cpu", description: "Studio process CPU utilization. 100% is one fully occupied core." }
                ResourceLane { id: chart_dom_id("gpu-utilization", &run.id), label: "GPU", value: gpu, chart: gpu_chart, class: "resource-gpu", description: "Whole-device Apple GPU utilization reported by macOS; other processes may contribute." }
                ResourceLane { id: chart_dom_id("memory-usage", &run.id), label: "Memory", value: memory, chart: memory_chart, class: "resource-memory", description: "Current resident memory used by the Studio process, including training allocations." }
            }
        }
    }
}

#[component]
fn ResourceLane(
    id: String,
    label: &'static str,
    value: String,
    chart: Chart,
    class: &'static str,
    description: &'static str,
) -> Element {
    rsx! {
        div { class: "training-resource-lane {class}", title: "{description}",
            div { span { "{label}" } strong { "{value}" } }
            CharmingChart {
                id,
                chart,
                class: "training-resource-chart".to_owned(),
                aria_label: format!("{label} by optimizer step. {description}"),
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ResourceSeries {
    Throughput,
    Transfer,
    Cpu,
    Gpu,
    Memory,
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

fn training_loss_chart(run: &TrainingRun) -> Chart {
    let train = metric_series_data(&run.metrics, |point| Some(point.train_loss), true);
    let validation = metric_series_data(&run.metrics, |point| point.validation_loss, true);
    let observed_steps = run
        .metrics
        .last()
        .map_or(run.current_step, |point| point.step.max(run.current_step))
        .max(1);

    Chart::new()
        .animation(false)
        .color(vec![COLOR_TRAIN, COLOR_VALIDATION])
        .tooltip(studio_numeric_tooltip(4))
        .grid(
            Grid::new()
                .left(48)
                .right(12)
                .top(12)
                .bottom(50)
                .contain_label(false),
        )
        .x_axis(
            chart_axis(AxisType::Value, true)
                .min(0)
                .max(observed_steps as f64)
                .split_number(4),
        )
        .y_axis(chart_axis(AxisType::Value, true).split_number(5))
        .data_zoom(
            DataZoom::new()
                .type_(DataZoomType::Inside)
                .x_axis_index(0)
                .filter_mode(FilterMode::Filter)
                .realtime(true)
                .throttle(50),
        )
        .data_zoom(
            DataZoom::new()
                .type_(DataZoomType::Slider)
                .x_axis_index(0)
                .filter_mode(FilterMode::Filter)
                .realtime(true)
                .brush_select(false)
                .left(48)
                .right(12)
                .bottom(4)
                .background_color(COLOR_SURFACE)
                .border_color(COLOR_LINE)
                .filler_color(COLOR_SELECTED),
        )
        .series(metric_line("train-loss", "Train", COLOR_TRAIN, train, 2.0))
        .series(metric_line(
            "validation-loss",
            "Validation",
            COLOR_VALIDATION,
            validation,
            1.8,
        ))
}

fn training_resource_chart(
    metrics: &[TrainingMetricPoint],
    total_steps: u64,
    series: ResourceSeries,
) -> Chart {
    let (id, name, color) = match series {
        ResourceSeries::Throughput => ("training-rate", "Training rate", COLOR_THROUGHPUT),
        ResourceSeries::Transfer => ("input-rate", "Input rate", COLOR_TRANSFER),
        ResourceSeries::Cpu => ("cpu-utilization", "CPU", COLOR_CPU),
        ResourceSeries::Gpu => ("gpu-utilization", "GPU", COLOR_GPU),
        ResourceSeries::Memory => ("memory-usage", "Memory", COLOR_MEMORY),
    };
    let data = metric_series_data(
        metrics,
        |point| match series {
            ResourceSeries::Throughput => Some(point.examples_per_second),
            ResourceSeries::Transfer => Some(point.transfer_bytes_per_second),
            ResourceSeries::Cpu => point.cpu_percent,
            ResourceSeries::Gpu => point.gpu_percent,
            ResourceSeries::Memory => point.memory_bytes.map(|value| value as f64),
        },
        false,
    );
    let tooltip = match series {
        ResourceSeries::Throughput => studio_rate_tooltip(TooltipRateUnit::SamplesPerSecond),
        ResourceSeries::Transfer => studio_rate_tooltip(TooltipRateUnit::BytesPerSecond),
        ResourceSeries::Cpu | ResourceSeries::Gpu => studio_percent_tooltip(),
        ResourceSeries::Memory => studio_bytes_tooltip(),
    };

    Chart::new()
        .animation(false)
        .color(vec![color])
        .tooltip(tooltip)
        .grid(Grid::new().left(0).right(0).top(6).bottom(2))
        .x_axis(
            chart_axis(AxisType::Value, false)
                .min(0)
                .max(total_steps.max(1) as f64),
        )
        .y_axis(match series {
            ResourceSeries::Gpu => chart_axis(AxisType::Value, false).min(0).max(100),
            _ => chart_axis(AxisType::Value, false).min(0).scale(true),
        })
        .series(
            metric_line(id, name, color, data, 1.5)
                .area_style(AreaStyle::new().color(color).opacity(0.09)),
        )
}

fn metric_series_data(
    metrics: &[TrainingMetricPoint],
    value: impl Fn(&TrainingMetricPoint) -> Option<f64>,
    positive_only: bool,
) -> DataFrame {
    let points = metrics
        .iter()
        .filter_map(|point| {
            let value = value(point)?;
            (value.is_finite()
                && (!positive_only || value > 0.0)
                && (positive_only || value >= 0.0))
                .then_some((point.step as f64, value))
        })
        .collect::<Vec<_>>();
    downsample_extrema(&points, TRAINING_CHART_POINT_BUDGET)
        .into_iter()
        .map(|(step, value)| xy_point(step, Some(value)))
        .collect()
}

fn metric_line(id: &str, name: &str, color: &str, data: DataFrame, width: f64) -> Line {
    Line::new()
        .id(id)
        .name(name)
        .show_symbol(false)
        .symbol(Symbol::None)
        .sampling(Sampling::Minmax)
        .connect_nulls(false)
        .line_style(LineStyle::new().color(color).width(width))
        .emphasis(Emphasis::new().disabled(true))
        .data(data)
}

fn chart_axis(kind: AxisType, visible: bool) -> Axis {
    Axis::new()
        .type_(kind)
        .show(true)
        .axis_line(AxisLine::new().show(false))
        .axis_tick(AxisTick::new().show(false))
        .axis_label(
            AxisLabel::new()
                .show(visible)
                .font_family(MONO_FONT)
                .font_size(10)
                .color(COLOR_INK_FAINT),
        )
        .split_line(
            SplitLine::new()
                .show(visible)
                .line_style(LineStyle::new().color(COLOR_LINE).width(1)),
        )
}

fn xy_point(x: f64, y: Option<f64>) -> DataPoint {
    DataPoint::from(vec![CompositeValue::from(x), CompositeValue::from(y)])
}

fn chart_dom_id(prefix: &str, key: &str) -> String {
    let safe = key
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("{prefix}-{safe}")
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

fn format_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GIB {
        format!("{:.2} GiB", bytes / GIB)
    } else {
        format!("{:.1} MiB", bytes / MIB)
    }
}

#[cfg(test)]
mod tests {
    use spiral_robotics_contracts::spiral::Table;

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
        let chart = training_resource_chart(&metrics, 100, ResourceSeries::Throughput);
        let option = serde_json::to_value(chart).expect("chart should serialize");
        assert_eq!(option["xAxis"]["max"], 100.0);
        assert_eq!(option["series"][0]["data"][0][0], 50.0);
        assert_eq!(step_percent(50, 100), 50.0);
    }

    #[test]
    fn loss_chart_uses_a_linear_axis_and_step_zoom_with_stable_series() {
        let run = TrainingRun {
            metrics: vec![TrainingMetricPoint {
                step: 1,
                train_loss: 0.25,
                validation_loss: Some(0.3),
                ..TrainingMetricPoint::default()
            }],
            ..TrainingRun::default()
        };
        let option =
            serde_json::to_value(training_loss_chart(&run)).expect("chart should serialize");

        assert_eq!(option["yAxis"]["type"], "value");
        assert_eq!(option["dataZoom"].as_array().map(Vec::len), Some(2));
        assert_eq!(option["dataZoom"][0]["type"], "inside");
        assert_eq!(option["dataZoom"][1]["type"], "slider");
        assert_eq!(option["dataZoom"][0]["xAxisIndex"], 0);
        assert_eq!(option["series"].as_array().map(Vec::len), Some(2));
        assert_eq!(option["series"][0]["id"], "train-loss");
        assert_eq!(option["series"][1]["id"], "validation-loss");
    }
}
