//! Training setup and run monitoring.

use crate::activities::data::dataset_catalog::DatasetCatalogFeedback;
use std::collections::BTreeSet;

use crate::components::interaction::{
    Action, ActionPurpose, ActionSize, ActionVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, DeleteIcon, InlineConfirmation,
};
use charming::Chart;
use charming::component::{Axis, DataZoom, DataZoomType, FilterMode, Grid};
use charming::datatype::{CompositeValue, DataFrame, DataPoint};
use charming::element::{
    AreaStyle, AxisLabel, AxisLine, AxisTick, AxisType, Emphasis, LineStyle, Sampling, SplitLine,
    Symbol,
};
use charming::series::Line;
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};
use spiral_robotics_common::identity::DatasetId;
use spiral_robotics_common::training::{
    TrainingMetricPoint, TrainingReadiness, TrainingRun, TrainingRunPhase,
};

#[cfg(test)]
use crate::activities::data::catalog as spiral;

use crate::components::charts::{
    COLOR_CPU, COLOR_GPU, COLOR_INK_FAINT, COLOR_LINE, COLOR_MEMORY, COLOR_SELECTED, COLOR_SURFACE,
    COLOR_THROUGHPUT, COLOR_TRAIN, COLOR_TRANSFER, COLOR_VALIDATION, CharmingChart, ChartPoints,
    MONO_FONT, StudioTooltip, TooltipRateUnit,
};
use crate::format::LossPresentation;
use crate::shell::state::StudioProjectState;

const EMPTY_RUN_PANEL_ID: &str = "training-run-empty";
const TRAINING_CHART_POINT_BUDGET: usize = 1_200;

#[component]
fn TrainingPanels(
    data: crate::activities::data::dataset_catalog::DatasetChoices,
    controller: super::state::TrainingController,
    training_history: super::history::TrainingHistory,
) -> Element {
    let snapshot = training_history.snapshot().read().to_owned();
    let run_names = snapshot
        .runs()
        .iter()
        .map(|run| run.config().run_name().to_owned())
        .collect::<Vec<_>>();
    use_effect(use_reactive((&run_names,), move |(names,)| {
        controller.sync_automatic_run_name(&names);
    }));
    let active_run = snapshot.active_run().cloned();
    let active_run_id = active_run.as_ref().map(|run| run.id().to_owned());
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
                data, controller, training_history,
            }
        },
    )
    .with_class("training-control-editor");
    let history = Panel::new(
        "training-history",
        "Runs",
        "training-history",
        rsx! { TrainingHistory { controller, history: snapshot.runs().to_owned() } },
    )
    .with_class("training-history-editor");

    let opened_ids = controller.opened_runs();
    let opened_runs = opened_ids
        .iter()
        .filter_map(|id| snapshot.runs().iter().find(|run| run.id() == id).cloned())
        .collect::<Vec<_>>();
    let registered_runs = opened_runs
        .iter()
        .map(|run| (run.id().training_panel_id(), run.id().to_owned()))
        .collect::<Vec<_>>();
    let mut panels = vec![control, history];
    let right_panel_ids = if opened_runs.is_empty() {
        panels.push(
            Panel::new(
                EMPTY_RUN_PANEL_ID,
                "Run",
                "training-runs",
                rsx! {
                    section { class: "training-run-empty",
                        strong { "Start training or select a run" }
                    }
                },
            )
            .with_class("training-run-editor"),
        );
        vec![EMPTY_RUN_PANEL_ID.to_owned()]
    } else {
        let ids = opened_runs
            .iter()
            .map(|run| run.id().training_panel_id())
            .collect::<Vec<_>>();
        for run in opened_runs {
            let panel_id = run.id().training_panel_id();
            panels.push(
                Panel::new(
                    panel_id,
                    run.config().run_name().to_owned(),
                    "training-runs",
                    rsx! {
                        TrainingRunDetail {
                            details_loaded: snapshot.detailed_run_id() == Some(run.id()),
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
        .filter(|id| snapshot.runs().iter().any(|run| run.id() == id))
        .map(|id| PanelId::from(id.training_panel_id()));
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
        if let Some(error) = training_history.error() {
            p { role: "alert", class: "p-3 text-xs text-danger", "{error}" }
        } else if !training_history.loaded() {
            p { role: "status", class: "p-3 text-xs text-ink-soft", "Loading training history…" }
        } else {
        PanelWorkspace {
            panels,
            initial_layout: layout,
            layout_scope: Some("training".to_owned()),
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
}

#[component]
fn TrainingRunControl(
    data: crate::activities::data::dataset_catalog::DatasetChoices,
    controller: super::state::TrainingController,
    training_history: super::history::TrainingHistory,
) -> Element {
    let studio = StudioProjectState::current();
    let dataset_id = data.selected().read().to_owned();
    let snapshot = training_history.snapshot().read().to_owned();
    let readiness = snapshot.readiness().to_owned();
    let active_run = snapshot.active_run().cloned();
    let mut selected_dataset = data.selected();
    let config = controller.config();
    let dataset_ids = data
        .datasets()
        .into_iter()
        .map(|dataset| dataset.id().to_owned())
        .chain(std::iter::once(dataset_id.clone()))
        .collect::<BTreeSet<_>>();
    let starting = controller.starting();
    let stopping = controller.stopping();
    let dataset_valid = DatasetId::is_canonical(dataset_id.trim());
    let can_start = readiness.ready() && active_run.is_none() && !starting && dataset_valid;
    let active_id = active_run.as_ref().map(|run| run.id().to_owned());
    let progress = active_run.as_ref().map(|run| {
        if run.config().steps() == 0 {
            0.0
        } else {
            run.current_step() as f64 / run.config().steps() as f64 * 100.0
        }
    });
    let (readiness_summary, readiness_recovery) = readiness.guidance();
    let runtime_summary = readiness
        .runtime()
        .as_ref()
        .map(|runtime| format!("{} · {}", runtime.display_name(), runtime.runtime()))
        .unwrap_or_else(|| "Runtime unavailable".to_owned());
    let mut runtime_devices = vec![("auto".to_owned(), "Auto".to_owned())];
    if let Some(runtime) = &readiness.runtime() {
        for label in runtime.devices() {
            let (value, label) = label.runtime_device_option();
            if !runtime_devices
                .iter()
                .any(|(existing, _)| existing == &value)
            {
                runtime_devices.push((value, label));
            }
        }
    }
    if !runtime_devices
        .iter()
        .any(|(value, _)| value == config.device())
    {
        runtime_devices.push((config.device().to_owned(), config.device().to_owned()));
    }
    let camera_summary = if config.act().camera_streams().is_empty() {
        "All available".to_owned()
    } else {
        format!(
            "{} · {}",
            config.act().camera_streams().len(),
            config.act().camera_streams().join(", ")
        )
    };
    let validation_percent = config.validation_split() * 100.0;

    rsx! {
        div { class: "training-control",
            div { class: "training-control-scroll",
            if !readiness.ready() {
                div { class: "training-inline-state training-inline-state-caution",
                    div { class: "training-inline-copy",
                        strong { "Training is not ready" }
                        span { "{readiness_summary}" }
                        p { "{readiness_recovery}" }
                        details { class: "technical-details",
                            summary { "Technical details" }
                            code { "{readiness.summary()}" }
                        }
                    }
                }
            }

            if let Some(run) = active_run.clone() {
                section { class: "training-active-run",
                    div { class: "training-active-heading",
                        div {
                            strong { "{run.config().run_name()}" }
                            span { class: run.phase().presentation_class(), "{run.phase().label()}" }
                            small { "Dataset · {run.config().dataset_id()}" }
                        }
                        span { class: "training-step-readout", "{run.current_step()} / {run.config().steps()}" }
                    }
                    div {
                        class: "training-progress-track",
                        role: "progressbar",
                        "aria-label": "Training progress",
                        "aria-valuemin": "0",
                        "aria-valuemax": "{run.config().steps()}",
                        "aria-valuenow": "{run.current_step()}",
                        "aria-valuetext": "Step {run.current_step()} of {run.config().steps()}",
                        div { class: "training-progress-fill", style: "width: {progress.unwrap_or_default().clamp(0.0, 100.0):.2}%" }
                    }
                    if run.phase() != TrainingRunPhase::Running {
                        if let Some(message) = run.message() {
                        p { "{message}" }
                        }
                    }
                    Action {
                        purpose: ActionPurpose::Stop,
                        variant: ActionVariant::Outline,
                        size: ActionSize::Standard,
                        class: "training-stop-action",
                        title: "Finish the current step, save a checkpoint, and stop",
                        disabled: run.phase() == TrainingRunPhase::Stopping || stopping.is_some(),
                        aria_busy: run.phase() == TrainingRunPhase::Stopping || stopping.is_some(),
                        onclick: move |_| {
                            let id = active_id.clone().unwrap_or_default();
                            controller.stop(studio, id);
                        },
                        if run.phase() == TrainingRunPhase::Stopping || stopping.is_some() { "Stopping…" } else { "Stop training" }
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
                DatasetCatalogFeedback { data }
                dl { class: "training-source-meta",
                    div { dt { "Cameras" } dd { "{camera_summary}" } }
                }
            }

            section { class: "training-form-section training-runtime-section",
                header { class: "training-form-heading",
                    h3 { "Compute" }
                }
                div { class: "training-runtime-summary",
                    span {
                        class: if readiness.runtime().is_some() { "training-runtime-dot training-runtime-dot-ready" } else { "training-runtime-dot" },
                        "aria-hidden": "true"
                    }
                    div {
                        strong { "{runtime_summary}" }
                        span {
                            if readiness.runtime().is_some() {
                                "Connected through Arrow IPC over HTTP"
                            } else {
                                "Connect a runtime in Settings"
                            }
                        }
                    }
                }
                label { class: "training-field training-field-wide",
                    span { "Device" }
                    select {
                        value: config.device().to_owned(),
                        disabled: active_run.is_some() || readiness.runtime().is_none(),
                        onchange: move |event| {
                            let value = event.value();
                            controller.edit_config(move |config| config.set_device(value));
                        },
                        for (value, label) in runtime_devices {
                            option { value: "{value}", "{label}" }
                        }
                    }
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
                            value: config.run_name().to_owned(),
                            disabled: active_run.is_some(),
                            oninput: move |event| {
                                controller.set_run_name(event.value());
                            },
                        }
                    }
                    label { class: "training-field",
                        span { "Steps" }
                        input {
                            r#type: "number", min: "1", step: "100",
                            value: "{config.steps()}",
                            disabled: active_run.is_some(),
                            oninput: move |event| if let Ok(value) = event.value().parse() {
                                controller.edit_config(move |config| config.set_steps(value));
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
                        span { "Batch" }
                        input {
                            r#type: "number", min: "1",
                            value: "{config.batch_size()}",
                            disabled: active_run.is_some(),
                            oninput: move |event| if let Ok(value) = event.value().parse() {
                                controller.edit_config(move |config| config.set_batch_size(value));
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
                                controller.edit_config(move |config| config.set_validation_split(value / 100.0));
                            },
                        }
                    }
                    label { class: "training-field",
                        span { "Action horizon" }
                        input {
                            r#type: "number", min: "1", max: "256",
                            value: "{config.chunk_size()}",
                            disabled: active_run.is_some(),
                            oninput: move |event| if let Ok(value) = event.value().parse() {
                                controller.edit_config(move |config| config.set_chunk_size(value));
                            },
                        }
                    }
                    label { class: "training-field",
                        span { "Learning rate" }
                        input {
                            r#type: "number", min: "0", step: "0.00001",
                            value: "{config.learning_rate()}",
                            disabled: active_run.is_some(),
                            oninput: move |event| if let Ok(value) = event.value().parse() {
                                controller.edit_config(move |config| config.set_learning_rate(value));
                            },
                        }
                    }
                    label { class: "training-field",
                        span { "Checkpoint interval" }
                        input {
                            r#type: "number", min: "1", step: "100",
                            value: "{config.checkpoint_every()}",
                            disabled: active_run.is_some(),
                            oninput: move |event| if let Ok(value) = event.value().parse() {
                                controller.edit_config(move |config| config.set_checkpoint_every(value));
                            },
                        }
                    }
                    }
                    div { class: "training-advanced-label", "Data query" }
                    textarea {
                        "aria-label": "Training data query",
                        spellcheck: "false",
                        value: config.sql().to_owned(),
                        disabled: active_run.is_some(),
                        oninput: move |event| {
                            let value = event.value();
                            controller.edit_config(move |config| config.set_sql(value));
                        },
                    }
                    div { class: "training-column-row",
                        label { class: "training-field",
                            span { "State column" }
                            input {
                                value: config.state_column().to_owned(),
                                disabled: active_run.is_some(),
                                oninput: move |event| {
                                    let value = event.value();
                                    controller.edit_config(move |config| config.set_state_column(value));
                                },
                            }
                        }
                        label { class: "training-field",
                            span { "Action column" }
                            input {
                                value: config.action_column().to_owned(),
                                disabled: active_run.is_some(),
                                oninput: move |event| {
                                    let value = event.value();
                                    controller.edit_config(move |config| config.set_action_column(value));
                                },
                            }
                        }
                    }
                    div { class: "training-column-row",
                        label { class: "training-field training-field-wide",
                            span { "Camera streams" }
                            input {
                                value: config.act().camera_streams().join(", "),
                                placeholder: "All available",
                                disabled: active_run.is_some(),
                                oninput: move |event| {
                                    let streams = event.value()
                                        .split(',')
                                        .map(str::trim)
                                        .filter(|stream| !stream.is_empty())
                                        .map(str::to_owned)
                                        .collect();
                                    controller.edit_config(move |config| config.act_mut().set_camera_streams(streams));
                                },
                            }
                        }
                        label { class: "training-field",
                            span { "Image width (0 = source)" }
                            input {
                                r#type: "number", min: "0", max: "1024", step: "1",
                                value: "{config.act().image_width()}",
                                disabled: active_run.is_some(),
                                oninput: move |event| if let Ok(value) = event.value().parse() {
                                    controller.edit_config(move |config| config.act_mut().set_image_width(value));
                                },
                            }
                        }
                        label { class: "training-field",
                            span { "Image height (0 = source)" }
                            input {
                                r#type: "number", min: "0", max: "1024", step: "1",
                                value: "{config.act().image_height()}",
                                disabled: active_run.is_some(),
                                oninput: move |event| if let Ok(value) = event.value().parse() {
                                    controller.edit_config(move |config| config.act_mut().set_image_height(value));
                                },
                            }
                        }
                    }
                }
            }
            }

            Action {
                purpose: ActionPurpose::Start,
                variant: ActionVariant::Solid,
                size: ActionSize::Prominent,
                class: "training-start-action",
                disabled: !can_start,
                aria_busy: starting,
                title: if active_run.is_some() { "Stop the active run first".to_owned() } else if !dataset_valid { "Choose a valid dataset ID first".to_owned() } else if !readiness.ready() { readiness_recovery.clone() } else { "Start training".to_owned() },
                onclick: move |_| {
                    controller.start(studio, training_history);
                },
                if starting { "Starting…" } else { "Run training" }
            }
        }
    }
}

#[component]
fn TrainingHistory(
    controller: super::state::TrainingController,
    history: Vec<TrainingRun>,
) -> Element {
    let confirming_delete = use_signal(|| None::<String>);
    rsx! {
        section { class: "training-history-panel studio-workbench-scroll",
            if history.is_empty() {
                div { class: "training-history-empty",
                    strong { "No runs yet" }
                    span { "Start training to create one." }
                }
            }
            for run in history {
                TrainingHistoryRow {
                    controller,
                    confirming_delete,
                    run,
                }
            }
        }
    }
}

#[component]
fn TrainingHistoryRow(
    run: TrainingRun,
    controller: super::state::TrainingController,
    mut confirming_delete: Signal<Option<String>>,
) -> Element {
    let studio = StudioProjectState::current();
    let selected = controller.selected_run().as_deref() == Some(run.id());
    let deleting_run = controller.deleting();
    let deleting = deleting_run.as_deref() == Some(run.id());
    let delete_disabled = run.phase().active() || deleting_run.is_some();
    let confirming = confirming_delete.read().as_deref() == Some(run.id());
    let checkpoint_word = if run.checkpoints().len() == 1 {
        "checkpoint"
    } else {
        "checkpoints"
    };
    let delete_title = if run.phase().active() {
        "Stop training before deleting this run."
    } else if deleting {
        "Deleting this run and its checkpoints."
    } else {
        "Delete this run and all of its checkpoints."
    };

    rsx! {
        article {
            class: "training-history-entry",
            Action {
                purpose: ActionPurpose::Select,
                variant: ActionVariant::Row,
                class: "training-history-row",
                aria_pressed: selected,
                title: "Open {run.config().run_name()}",
                onclick: {
                    let id = run.id().to_owned();
                    move |_| controller.open_run(id.clone())
                },
                span { class: "training-history-name", "{run.config().run_name()}" }
                span { class: "training-history-dataset", "{run.config().dataset_id()}" }
                span { class: run.phase().presentation_class(), "{run.phase().label()}" }
                span { class: "training-history-step", "{run.current_step()} / {run.config().steps()}" }
            }
            Action {
                id: "delete-run-{run.id()}",
                purpose: ActionPurpose::Delete,
                variant: ActionVariant::Icon,
                class: "training-history-delete",
                disabled: delete_disabled,
                title: "{delete_title}",
                aria_label: "Delete run {run.config().run_name()}",
                onclick: {
                    let id = run.id().to_owned();
                    move |_| {
                        if controller.deleting().is_none() {
                            confirming_delete.set(Some(id.clone()));
                        }
                    }
                },
                DeleteIcon {}
            }
            if confirming {
                InlineConfirmation {
                    id: "delete-run-confirmation-{run.id()}",
                    class: "training-history-delete-confirmation",
                    copy: ConfirmationCopy::new(
                        format!(
                            "Delete {} and its {} {}? Metrics and logs will be permanently removed.",
                            run.config().run_name(),
                            run.checkpoints().len(),
                            checkpoint_word,
                        ),
                        if deleting { "Deleting…" } else { "Delete run" },
                        "Keep run",
                    ),
                    response: ConfirmationResponse::new(
                        ActionPurpose::Delete,
                        {
                            let id = run.id().to_owned();
                            let name = run.config().run_name().to_owned();
                            move |_| {
                                confirming_delete.set(None);
                                controller.delete(
                                    studio,
                                    super::state::TrainingRunIdentity::new(id.clone(), name.clone()),
                                )
                            }
                        },
                        move |_| confirming_delete.set(None),
                    ),
                    dismissal: ConfirmationDismissal::new(format!("delete-run-{}", run.id()))
                        .with_pending(deleting),
                }
            }
        }
    }
}

#[component]
fn TrainingRunDetail(run: TrainingRun, details_loaded: bool) -> Element {
    if !details_loaded {
        return rsx! { article { class: "training-run-detail", role: "status", "Loading run details…" } };
    }
    let progress = if run.config().steps() == 0 {
        0.0
    } else {
        run.current_step() as f64 / run.config().steps() as f64 * 100.0
    };
    let message = run.message().to_owned();
    let logs = run.logs().to_owned();
    let message_class = if run.phase() == TrainingRunPhase::Failed {
        "training-run-message training-run-message-danger"
    } else {
        "training-run-message"
    };
    rsx! {
        article { class: "training-run-detail",
            header { class: "training-run-overview",
                div { class: "training-run-identity",
                    div {
                        h2 { "{run.config().run_name()}" }
                        span { class: run.phase().presentation_class(), "{run.phase().label()}" }
                    }
                    p { "Dataset · {run.config().dataset_id()} · {run.config().policy().label()} · {run.config().device()}" }
                }
                dl { class: "training-run-facts",
                    div { dt { "Step" } dd { "{run.current_step()} / {run.config().steps()}" } }
                    div { dt { "Progress" } dd { "{progress.clamp(0.0, 100.0):.1}%" } }
                    div { dt { "Checkpoints" } dd { "{run.checkpoints().len()}" } }
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

trait TrainingIdentityPresentation {
    fn training_panel_id(&self) -> String;
    fn chart_dom_id(&self, prefix: &str) -> String;
    fn training_chart_group(&self) -> String;
}

impl TrainingIdentityPresentation for str {
    fn training_panel_id(&self) -> String {
        format!("training-run-{self}")
    }

    fn chart_dom_id(&self, prefix: &str) -> String {
        let safe = self
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

    fn training_chart_group(&self) -> String {
        self.chart_dom_id("training-step-window")
    }
}

trait TrainingReadinessPresentation {
    fn guidance(&self) -> (String, String);
}

impl TrainingReadinessPresentation for TrainingReadiness {
    fn guidance(&self) -> (String, String) {
        if !self.spiral_ready() {
            return (
                "SpiralDB is unavailable.".to_owned(),
                "Open Settings and test the SpiralDB connection.".to_owned(),
            );
        }
        if self.runtime().is_none() {
            return (
                self.summary().to_owned(),
                "Open Settings and connect the policy compute runtime.".to_owned(),
            );
        }
        if !self.ready() {
            return (
                self.summary().to_owned(),
                "Resolve the reported blocker, then retry training.".to_owned(),
            );
        }
        (self.summary().to_owned(), "Training is ready.".to_owned())
    }
}

#[component]
fn TrainingLearning(run: TrainingRun) -> Element {
    let last = run.metrics().last();
    let latest_train = last.map(|point| point.train_loss().loss_label());
    let latest_validation = run
        .metrics()
        .iter()
        .rev()
        .find_map(|point| point.validation_loss())
        .map(LossPresentation::loss_label);
    let lr = last.map(|point| format!("{:.2e}", point.learning_rate()));
    let latest_train = latest_train.unwrap_or_else(|| "—".to_owned());
    let latest_validation = latest_validation.unwrap_or_else(|| "—".to_owned());
    let lr = lr.unwrap_or_else(|| "—".to_owned());
    let chart_id = run.id().chart_dom_id("training-loss");
    let chart_group = run.id().training_chart_group();
    let loss_chart = run.loss_chart();

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
                if run.metrics().is_empty() {
                    div { class: "training-chart-empty",
                        if run.phase() == TrainingRunPhase::Failed {
                            "No metrics."
                        } else if run.phase().active() {
                            "Waiting for step 1…"
                        } else {
                            "No metrics."
                        }
                    }
                }
                CharmingChart {
                    id: chart_id,
                    chart: loss_chart,
                    presentation: crate::components::charts::ChartPresentation::new(
                        "training-loss-chart",
                        "Training and validation loss by optimizer step",
                    )
                    .with_group(chart_group),
                }
            }
            div { class: "training-event-strip",
                div { class: "training-event-track",
                    i { class: "training-event-axis" }
                    for checkpoint in &run.checkpoints() {
                        span {
                            class: "training-event-tick training-event-checkpoint",
                            style: "left: {checkpoint.step().percent_of(run.config().steps()):.3}%",
                            title: "Checkpoint saved at step {checkpoint.step()}",
                            "aria-label": "Checkpoint at step {checkpoint.step()}",
                        }
                    }
                    for point in run.metrics().iter().filter(|point| point.event().is_some()) {
                        span {
                            class: if point.event().is_some_and(|event| event.contains("loader stall")) { "training-event-tick training-event-stall" } else { "training-event-tick training-event-validation" },
                            style: "left: {point.step().percent_of(run.config().steps()):.3}%",
                            title: point.event_label(),
                            "aria-label": point.event_label(),
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
    let cameras = if run.config().act().camera_streams().is_empty() {
        "All available".to_owned()
    } else {
        run.config().act().camera_streams().join(", ")
    };
    let image_size = match (
        run.config().act().image_width(),
        run.config().act().image_height(),
    ) {
        (0, 0) => "Source size".to_owned(),
        (0, height) => format!("Source width × {height}"),
        (width, 0) => format!("{width} × source height"),
        (width, height) => format!("{width} × {height}"),
    };
    let validation = if run.config().validation_split() == 0.0 {
        "Off".to_owned()
    } else {
        format!("{:.0}% · episode", run.config().validation_split() * 100.0)
    };
    rsx! {
        section { class: "training-inputs",
            dl { class: "training-input-facts",
                div { class: "training-input-wide",
                    dt { "Cameras" }
                    dd { "{cameras}" }
                }
                div { dt { "Batch" } dd { "{run.config().batch_size()} samples" } }
                div { dt { "Validation" } dd { "{validation}" } }
                div { dt { "Action horizon" } dd { "{run.config().chunk_size()} steps" } }
                div { dt { "Image size" } dd { "{image_size}" } }
                div { dt { "Checkpoint interval" } dd { "{run.config().checkpoint_every()} steps" } }
            }
        }
    }
}

#[component]
fn TrainingPerformance(run: TrainingRun) -> Element {
    let metrics = run.metrics();
    let last = metrics.last();
    let observed_steps = last
        .map_or(run.current_step(), |point| {
            point.step().max(run.current_step())
        })
        .max(1);
    let chart_group = run.id().training_chart_group();
    let throughput = last
        .map(|point| format!("{:.1} samples/s", point.examples_per_second()))
        .unwrap_or_else(|| "—".to_owned());
    let transfer = last
        .map(|point| point.transfer_bytes_per_second().formatted_rate())
        .unwrap_or_else(|| "—".to_owned());
    let cpu = last
        .and_then(|point| point.cpu_percent())
        .map(|value| format!("{value:.1}%"))
        .unwrap_or_else(|| "—".to_owned());
    let gpu = last
        .and_then(|point| point.gpu_percent())
        .map(|value| format!("{value:.1}%"))
        .unwrap_or_else(|| "—".to_owned());
    let memory = last
        .and_then(|point| point.memory_bytes())
        .map(TrainingByteCountPresentation::formatted_bytes)
        .unwrap_or_else(|| "—".to_owned());
    let loader_ms = last
        .and_then(|point| point.loader_ms())
        .formatted_milliseconds();
    let arrow_encode_ms = last
        .and_then(|point| point.arrow_encode_ms())
        .formatted_milliseconds();
    let upload_ms = last
        .and_then(|point| point.upload_ms())
        .formatted_milliseconds();
    let device_copy_ms = last
        .and_then(|point| point.device_copy_ms())
        .formatted_milliseconds();
    let model_ms = last
        .and_then(|point| point.model_ms())
        .formatted_milliseconds();
    let throughput_chart = ResourceSeries::Throughput.chart(metrics, observed_steps);
    let transfer_chart = ResourceSeries::Transfer.chart(metrics, observed_steps);
    let cpu_chart = ResourceSeries::Cpu.chart(metrics, observed_steps);
    let gpu_chart = ResourceSeries::Gpu.chart(metrics, observed_steps);
    let memory_chart = ResourceSeries::Memory.chart(metrics, observed_steps);

    rsx! {
        section { class: "training-resources",
            dl { class: "training-phase-timings", "aria-label": "Latest optimizer step timing",
                div { title: "Spiral query, frame decode, normalization, and batch assembly. Prefetch overlaps this work with the preceding optimizer step.",
                    dt { "Load" }
                    dd { "{loader_ms}" }
                }
                div { title: "Studio time to materialize the batch as an Arrow IPC stream.",
                    dt { "Arrow encode" }
                    dd { "{arrow_encode_ms}" }
                }
                div { title: "HTTP request and response time outside the remote runtime handler, dominated by request-body upload for remote runtimes.",
                    dt { "Upload" }
                    dd { "{upload_ms}" }
                }
                div { title: "Runtime tensor conversion and host-to-device transfer.",
                    dt { "Device copy" }
                    dd { "{device_copy_ms}" }
                }
                div { title: "Runtime forward pass, backward pass, gradient clipping, and optimizer step.",
                    dt { "Model" }
                    dd { "{model_ms}" }
                }
            }
            div { class: "training-resource-grid",
                ResourceLane { lane: ResourceLaneModel::new(run.id().chart_dom_id("training-rate"), "Training rate", throughput).with_chart(throughput_chart, chart_group.clone()).with_presentation("resource-throughput", "Samples completed per second.") }
                ResourceLane { lane: ResourceLaneModel::new(run.id().chart_dom_id("input-rate"), "Input rate", transfer).with_chart(transfer_chart, chart_group.clone()).with_presentation("resource-transfer", "Prepared camera data per second.") }
                ResourceLane { lane: ResourceLaneModel::new(run.id().chart_dom_id("cpu-utilization"), "CPU", cpu).with_chart(cpu_chart, chart_group.clone()).with_presentation("resource-cpu", "Runtime process CPU utilization. 100% is one fully occupied core.") }
                ResourceLane { lane: ResourceLaneModel::new(run.id().chart_dom_id("gpu-utilization"), "GPU", gpu).with_chart(gpu_chart, chart_group.clone()).with_presentation("resource-gpu", "Whole-device utilization reported by the selected runtime; other processes may contribute.") }
                ResourceLane { lane: ResourceLaneModel::new(run.id().chart_dom_id("memory-usage"), "Memory", memory).with_chart(memory_chart, chart_group).with_presentation("resource-memory", "Current resident memory used by the runtime process, including training allocations.") }
            }
        }
    }
}

#[derive(Clone, PartialEq)]
struct ResourceLaneModel {
    id: String,
    label: &'static str,
    value: String,
    chart: Chart,
    group: String,
    class: &'static str,
    description: &'static str,
}

impl ResourceLaneModel {
    fn new(id: String, label: &'static str, value: String) -> Self {
        Self {
            id,
            label,
            value,
            chart: Chart::new(),
            group: String::new(),
            class: "",
            description: "",
        }
    }

    fn with_chart(mut self, chart: Chart, group: String) -> Self {
        self.chart = chart;
        self.group = group;
        self
    }

    fn with_presentation(mut self, class: &'static str, description: &'static str) -> Self {
        self.class = class;
        self.description = description;
        self
    }
}

#[component]
fn ResourceLane(lane: ResourceLaneModel) -> Element {
    let ResourceLaneModel {
        id,
        label,
        value,
        chart,
        group,
        class,
        description,
    } = lane;
    rsx! {
        div { class: "training-resource-lane {class}", title: "{description}",
            div { span { "{label}" } strong { "{value}" } }
            CharmingChart {
                id,
                chart,
                presentation: crate::components::charts::ChartPresentation::new(
                    "training-resource-chart",
                    format!("{label} by optimizer step. {description}"),
                )
                .with_group(group),
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

trait TrainingPhasePresentation {
    fn presentation_class(self) -> &'static str;
}

impl TrainingPhasePresentation for TrainingRunPhase {
    fn presentation_class(self) -> &'static str {
        match self {
            TrainingRunPhase::Starting | TrainingRunPhase::Running | TrainingRunPhase::Stopping => {
                "training-phase training-phase-active"
            }
            TrainingRunPhase::Completed => "training-phase training-phase-complete",
            TrainingRunPhase::Stopped => "training-phase",
            TrainingRunPhase::Failed => "training-phase training-phase-failed",
        }
    }
}

trait TrainingRunPresentation {
    fn loss_chart(&self) -> Chart;
}

impl TrainingRunPresentation for TrainingRun {
    fn loss_chart(&self) -> Chart {
        let train = self
            .metrics()
            .series_data(|point| Some(point.train_loss()), true);
        let validation = self
            .metrics()
            .series_data(|point| point.validation_loss(), true);
        let observed_steps = self
            .metrics()
            .last()
            .map_or(self.current_step(), |point| {
                point.step().max(self.current_step())
            })
            .max(1);

        Chart::new()
            .animation(false)
            .color(vec![COLOR_TRAIN, COLOR_VALIDATION])
            .tooltip(StudioTooltip::numeric(4))
            .grid(
                Grid::new()
                    .left(48)
                    .right(12)
                    .top(12)
                    .bottom(50)
                    .contain_label(false),
            )
            .x_axis(
                TrainingChartStyle::axis(AxisType::Value, true)
                    .min(0)
                    .max(observed_steps as f64)
                    .split_number(4),
            )
            .y_axis(TrainingChartStyle::axis(AxisType::Value, true).split_number(5))
            .data_zoom(TrainingChartStyle::slider_zoom(48.0, 12.0))
            .series(
                MetricLineSpec::new("train-loss", "Train", COLOR_TRAIN)
                    .with_width(2.0)
                    .render(train),
            )
            .series(
                MetricLineSpec::new("validation-loss", "Validation", COLOR_VALIDATION)
                    .with_width(1.8)
                    .render(validation),
            )
    }
}

impl ResourceSeries {
    fn chart(self, metrics: &[TrainingMetricPoint], observed_steps: u64) -> Chart {
        let (id, name, color) = match self {
            Self::Throughput => ("training-rate", "Training rate", COLOR_THROUGHPUT),
            Self::Transfer => ("input-rate", "Input rate", COLOR_TRANSFER),
            Self::Cpu => ("cpu-utilization", "CPU", COLOR_CPU),
            Self::Gpu => ("gpu-utilization", "GPU", COLOR_GPU),
            Self::Memory => ("memory-usage", "Memory", COLOR_MEMORY),
        };
        let data = metrics.series_data(
            |point| match self {
                Self::Throughput => Some(point.examples_per_second()),
                Self::Transfer => Some(point.transfer_bytes_per_second()),
                Self::Cpu => point.cpu_percent(),
                Self::Gpu => point.gpu_percent(),
                Self::Memory => point.memory_bytes().map(|value| value as f64),
            },
            false,
        );
        let tooltip = match self {
            Self::Throughput => StudioTooltip::rate(TooltipRateUnit::SamplesPerSecond),
            Self::Transfer => StudioTooltip::rate(TooltipRateUnit::BytesPerSecond),
            Self::Cpu | Self::Gpu => StudioTooltip::percent(),
            Self::Memory => StudioTooltip::bytes(),
        };

        Chart::new()
            .animation(false)
            .color(vec![color])
            .tooltip(tooltip)
            .grid(Grid::new().left(0).right(0).top(6).bottom(40))
            .x_axis(
                TrainingChartStyle::axis(AxisType::Value, false)
                    .min(0)
                    .max(observed_steps.max(1) as f64),
            )
            .y_axis(match self {
                Self::Gpu => TrainingChartStyle::axis(AxisType::Value, false)
                    .min(0)
                    .max(100),
                _ => TrainingChartStyle::axis(AxisType::Value, false)
                    .min(0)
                    .scale(true),
            })
            .data_zoom(TrainingChartStyle::slider_zoom(8.0, 8.0))
            .series(
                MetricLineSpec::new(id, name, color)
                    .with_width(1.5)
                    .render(data)
                    .area_style(AreaStyle::new().color(color).opacity(0.09)),
            )
    }
}

trait TrainingMetricSeries {
    fn series_data(
        &self,
        value: impl Fn(&TrainingMetricPoint) -> Option<f64>,
        positive_only: bool,
    ) -> DataFrame;
}

impl TrainingMetricSeries for [TrainingMetricPoint] {
    fn series_data(
        &self,
        value: impl Fn(&TrainingMetricPoint) -> Option<f64>,
        positive_only: bool,
    ) -> DataFrame {
        let points = self
            .iter()
            .filter_map(|point| {
                let value = value(point)?;
                (value.is_finite()
                    && (!positive_only || value > 0.0)
                    && (positive_only || value >= 0.0))
                    .then_some((point.step() as f64, value))
            })
            .collect::<Vec<_>>();
        points
            .downsample_extrema(TRAINING_CHART_POINT_BUDGET)
            .into_iter()
            .map(|(step, value)| TrainingChartStyle::point(step, Some(value)))
            .collect()
    }
}

struct MetricLineSpec<'a> {
    id: &'a str,
    name: &'a str,
    color: &'a str,
    width: f64,
}

impl<'a> MetricLineSpec<'a> {
    const fn new(id: &'a str, name: &'a str, color: &'a str) -> Self {
        Self {
            id,
            name,
            color,
            width: 1.0,
        }
    }

    const fn with_width(mut self, width: f64) -> Self {
        self.width = width;
        self
    }

    fn render(self, data: DataFrame) -> Line {
        Line::new()
            .id(self.id)
            .name(self.name)
            .show_symbol(false)
            .symbol(Symbol::None)
            .sampling(Sampling::Minmax)
            .connect_nulls(false)
            .line_style(LineStyle::new().color(self.color).width(self.width))
            .emphasis(Emphasis::new().disabled(true))
            .data(data)
    }
}

struct TrainingChartStyle;

impl TrainingChartStyle {
    fn slider_zoom(left: f64, right: f64) -> DataZoom {
        DataZoom::new()
            .type_(DataZoomType::Slider)
            .show(true)
            .x_axis_index(0)
            .filter_mode(FilterMode::Filter)
            .realtime(true)
            .throttle(50)
            .brush_select(false)
            .left(left)
            .right(right)
            .bottom(4)
            .background_color(COLOR_SURFACE)
            .border_color(COLOR_LINE)
            .filler_color(COLOR_SELECTED)
    }

    fn axis(kind: AxisType, visible: bool) -> Axis {
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

    fn point(x: f64, y: Option<f64>) -> DataPoint {
        DataPoint::from(vec![CompositeValue::from(x), CompositeValue::from(y)])
    }
}

trait TrainingStepPresentation {
    fn percent_of(self, total_steps: u64) -> f64;
}

impl TrainingStepPresentation for u64 {
    fn percent_of(self, total_steps: u64) -> f64 {
        (self as f64 / total_steps.max(1) as f64 * 100.0).clamp(0.0, 100.0)
    }
}

trait TrainingMetricEventPresentation {
    fn event_label(&self) -> String;
}

impl TrainingMetricEventPresentation for TrainingMetricPoint {
    fn event_label(&self) -> String {
        format!(
            "{} at step {}",
            self.event().unwrap_or("Event"),
            self.step()
        )
    }
}

trait TrainingByteRatePresentation {
    fn formatted_rate(self) -> String;
}

impl TrainingByteRatePresentation for f64 {
    fn formatted_rate(self) -> String {
        const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
        const MIB: f64 = 1024.0 * 1024.0;
        if self >= GIB {
            format!("{:.2} GiB/s", self / GIB)
        } else {
            format!("{:.1} MiB/s", self / MIB)
        }
    }
}

trait TrainingByteCountPresentation {
    fn formatted_bytes(self) -> String;
}

impl TrainingByteCountPresentation for u64 {
    fn formatted_bytes(self) -> String {
        const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
        const MIB: f64 = 1024.0 * 1024.0;
        let bytes = self as f64;
        if bytes >= GIB {
            format!("{:.2} GiB", bytes / GIB)
        } else {
            format!("{:.1} MiB", bytes / MIB)
        }
    }
}

trait TrainingDurationPresentation {
    fn formatted_milliseconds(self) -> String;
}

impl TrainingDurationPresentation for Option<f64> {
    fn formatted_milliseconds(self) -> String {
        self.filter(|value| value.is_finite() && *value >= 0.0)
            .map(|value| {
                if value >= 1_000.0 {
                    format!("{:.2} s", value / 1_000.0)
                } else if value >= 10.0 {
                    format!("{value:.0} ms")
                } else {
                    format!("{value:.1} ms")
                }
            })
            .unwrap_or_else(|| "—".to_owned())
    }
}

/// Runtime health responses keep device discovery intentionally lightweight:
/// the first whitespace-delimited token is the identifier accepted by session
/// creation and the complete string is its operator-facing label.
trait RuntimeDevicePresentation {
    fn runtime_device_option(&self) -> (String, String);
}

impl RuntimeDevicePresentation for str {
    fn runtime_device_option(&self) -> (String, String) {
        let label = self.trim();
        let value = label
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        (value, label.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use crate::activities::data::catalog::ManagedTable as Table;

    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn phase_timings_keep_submillisecond_detail_and_scan_long_uploads_as_seconds() {
        assert_eq!(Some(0.26).formatted_milliseconds(), "0.3 ms");
        assert_eq!(Some(149.4).formatted_milliseconds(), "149 ms");
        assert_eq!(Some(2_963.0).formatted_milliseconds(), "2.96 s");
        assert_eq!(None.formatted_milliseconds(), "—");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn runtime_device_options_submit_identifiers_instead_of_hardware_labels() {
        assert_eq!(
            "CUDA:0 NVIDIA A100-SXM4-40GB".runtime_device_option(),
            (
                "cuda:0".to_owned(),
                "CUDA:0 NVIDIA A100-SXM4-40GB".to_owned()
            )
        );
        assert_eq!(
            "CPU".runtime_device_option(),
            ("cpu".to_owned(), "CPU".to_owned())
        );
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn default_query_selects_complete_kept_follower_samples() {
        let sql = spiral::ManagedTable::Datasets.training_sql_for_dataset("robotics", "pick-place");
        assert!(sql.contains(&format!(
            "FROM {}",
            Table::JointSamples.qualified_name("robotics")
        )));
        assert!(sql.contains(&Table::Episodes.qualified_name("robotics")));
        assert!(sql.contains(&Table::EpisodeStreams.qualified_name("robotics")));
        assert!(sql.contains(&format!("j.{}", spiral::column::STATE_VALID)));
        assert!(sql.contains(&format!("j.{}", spiral::column::ACTION_VALID)));
        assert!(sql.contains("LIKE '%follower%'"));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn unavailable_training_never_claims_to_be_ready() {
        let readiness =
            TrainingReadiness::new(false, true, "Policy compute runtime is unavailable.");
        let (summary, recovery) = readiness.guidance();
        assert_eq!(summary, "Policy compute runtime is unavailable.");
        assert_eq!(
            recovery,
            "Open Settings and connect the policy compute runtime."
        );

        let checking = TrainingReadiness::new(false, true, "Checking training storage.")
            .with_runtime(Some(Default::default()));
        assert_ne!(checking.guidance().1, "Training is ready.");
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn throughput_uses_the_optimizer_step_axis_and_slider_zoom() {
        let metrics = vec![TrainingMetricPoint::new(50, 0.0).with_throughput(0.0, 25.0)];
        let chart = ResourceSeries::Throughput.chart(&metrics, 100);
        let option = serde_json::to_value(chart).expect("chart should serialize");
        assert_eq!(option["xAxis"]["max"], 100.0);
        assert_eq!(option["series"][0]["data"][0][0], 50.0);
        assert_eq!(option["dataZoom"].as_array().map(Vec::len), Some(1));
        assert_eq!(option["dataZoom"][0]["type"], "slider");
        assert_eq!(option["dataZoom"][0]["show"], true);
        assert_eq!(option["dataZoom"][0]["xAxisIndex"], 0);
        assert_eq!(50_u64.percent_of(100), 50.0);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn loss_chart_uses_a_linear_axis_and_slider_zoom_with_stable_series() {
        let run = TrainingRun::default().with_metrics(vec![
            TrainingMetricPoint::new(1, 0.25).with_validation_loss(Some(0.3)),
        ]);
        let option = serde_json::to_value(run.loss_chart()).expect("chart should serialize");

        assert_eq!(option["yAxis"]["type"], "value");
        assert_eq!(option["dataZoom"].as_array().map(Vec::len), Some(1));
        assert_eq!(option["dataZoom"][0]["type"], "slider");
        assert_eq!(option["dataZoom"][0]["xAxisIndex"], 0);
        assert_eq!(option["series"].as_array().map(Vec::len), Some(2));
        assert_eq!(option["series"][0]["id"], "train-loss");
        assert_eq!(option["series"][1]["id"], "validation-loss");
    }
}

#[component]
pub(crate) fn TrainingWorkspace(is_active: ReadSignal<bool>) -> Element {
    let workspace = crate::shell::activity::WorkspaceActivity::provide(
        crate::shell::activity::Activity::Training,
        is_active,
    );
    let studio = crate::shell::state::StudioProjectState::current();
    let choices = crate::activities::data::dataset_catalog::DatasetChoices::use_choices();
    let controller = super::state::TrainingController::use_controller(
        studio.configuration().dataset(),
        &choices.selected().read(),
    );
    let history =
        super::history::TrainingHistory::use_history(is_active, move || controller.selected_run());
    rsx! { crate::shell::frame::WorkspaceFrame { workspace, status: crate::shell::frame::WorkspaceStatus::Training(history), TrainingPanels { data: choices, controller, training_history: history } } }
}
