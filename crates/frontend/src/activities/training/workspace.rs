//! Training setup and run monitoring.

use std::collections::BTreeSet;

use crate::components::icons::TrainingIcon;
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
    TrainingCheckpoint, TrainingMetricPoint, TrainingReadiness, TrainingRun, TrainingRunPhase,
};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, DeleteIcon, Disclosure, EmptyState, Fact, FactLayout, FactList, Field,
    InlineConfirmation, Select, TextInput, TextInputKind,
};

use crate::components::charts::{CharmingChart, ChartColor, ChartPoints, MONO_FONT, StudioTooltip};
use crate::components::dataset_picker::DatasetCatalog;
use crate::format::{ByteCountPresentation, LossPresentation, MillisecondPresentation};
use crate::shell::state::StudioProjectState;

const EMPTY_RUN_PANEL_ID: &str = "training-run-empty";
const TRAINING_CHART_POINT_BUDGET: usize = 1_200;

#[component]
fn TrainingPanels(
    project: StudioProjectState,
    controller: super::state::TrainingController,
    training_history: super::history::TrainingHistory,
) -> Element {
    let snapshot = training_history.snapshot().read().to_owned();
    let checkpoints = training_history.checkpoints().read().to_owned();
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
            TrainingRunControl { project, controller, training_history }
        },
    )
    .with_class("training-control-editor");
    let history = Panel::new(
        "training-history",
        "Runs",
        "training-history",
        rsx! { TrainingHistory { controller, history: snapshot.runs().to_owned(), checkpoints: checkpoints.clone() } },
    )
    .with_class("training-history-editor");

    let opened_ids = controller.opened_runs();
    let opened_runs = opened_ids
        .iter()
        .filter_map(|id| snapshot.runs().iter().find(|run| run.id() == id).cloned())
        .collect::<Vec<_>>();
    let registered_runs = opened_runs
        .iter()
        .map(|run| (run.panel_id(), run.id().to_owned()))
        .collect::<Vec<_>>();
    let mut panels = vec![control, history];
    let right_panel_ids = if opened_runs.is_empty() {
        panels.push(
            Panel::new(
                EMPTY_RUN_PANEL_ID,
                "Run",
                "training-runs",
                rsx! {
                    EmptyState { icon: rsx! { TrainingIcon {} }, title: "No run open",
                        p { "Choose a run from Runs, or start one from Setup, to follow its learning curve, resources, and checkpoints here." }
                    }
                },
            )
            .with_class("training-run-editor"),
        );
        vec![EMPTY_RUN_PANEL_ID.to_owned()]
    } else {
        let ids = opened_runs
            .iter()
            .map(|run| run.panel_id())
            .collect::<Vec<_>>();
        for run in opened_runs {
            let panel_id = run.panel_id();
            let checkpoints = checkpoints.of_run(run.id());
            panels.push(
                Panel::new(
                    panel_id,
                    run.config().run_name().to_owned(),
                    "training-runs",
                    rsx! {
                        TrainingRunDetail {
                            details_loaded: training_history.details_loaded(run.id()),
                            run,
                            checkpoints,
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
        .and_then(|id| snapshot.runs().iter().find(|run| run.id() == id))
        .map(|run| PanelId::from(run.panel_id()));
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
            on_panel_activate: move |panel_id: PanelId| {
                if let Some((_, run_id)) = registered_runs
                    .iter()
                    .find(|(registered, _)| registered == panel_id.as_str())
                {
                    controller.select_run(run_id.clone());
                }
            },
            on_panel_close: move |panel: PanelId| {
                if let Some((_, run_id)) = closeable_runs
                    .iter()
                    .find(|(registered, _)| registered == panel.as_str())
                {
                    controller.close_run(run_id.clone());
                }
            },
        }
        }
    }
}

#[component]
fn TrainingRunControl(
    project: StudioProjectState,
    controller: super::state::TrainingController,
    training_history: super::history::TrainingHistory,
) -> Element {
    let catalog = DatasetCatalog::use_catalog();
    let snapshot = training_history.snapshot().read().to_owned();
    let readiness = snapshot.readiness().to_owned();
    let active_run = snapshot.active_run().cloned();
    let config = controller.config();
    let dataset_id = config.dataset_id().to_owned();
    let dataset_ids = catalog
        .datasets()
        .into_iter()
        .filter_map(|dataset| {
            dataset
                .selection()
                .label()
                .strip_prefix("dataset_")
                .map(str::to_owned)
        })
        .chain(std::iter::once(dataset_id.clone()))
        .collect::<BTreeSet<_>>();
    let starting = controller.starting();
    let stopping = controller.stopping();
    let dataset_valid = DatasetId::is_canonical(dataset_id.trim());
    let training_blocker = config.policy().mcap_training_blocker();
    let can_start = training_blocker.is_none()
        && readiness.ready()
        && active_run.is_none()
        && !starting
        && dataset_valid;
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
            if let Some(reason) = training_blocker {
                div { class: "training-inline-state training-inline-state-caution", role: "status",
                    div { class: "training-inline-copy",
                        strong { "MCAP training is not available yet" }
                        span { "{reason}" }
                        p { "You can record episodes and play them back in Data. Existing training history and checkpoints remain available." }
                    }
                }
            } else if !readiness.ready() {
                div { class: "training-inline-state training-inline-state-caution",
                    div { class: "training-inline-copy",
                        strong { "Training is not ready" }
                        span { "{readiness_summary}" }
                        p { "{readiness_recovery}" }
                        Disclosure { summary: rsx! { "Technical details" },
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
                            span { class: run.phase_class(), "{run.phase().label()}" }
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
                    Button {
                        purpose: ActionPurpose::Stop,
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Standard,
                        class: "training-stop-action",
                        title: "Finish the current step, save a checkpoint, and stop",
                        disabled: run.phase() == TrainingRunPhase::Stopping || stopping.is_some(),
                        aria_busy: run.phase() == TrainingRunPhase::Stopping || stopping.is_some(),
                        onclick: move |_| {
                            let id = active_id.clone().unwrap_or_default();
                            controller.stop(project, id);
                        },
                        if run.phase() == TrainingRunPhase::Stopping || stopping.is_some() { "Stopping…" } else { "Stop training" }
                    }
                }
            }

            section { class: "training-form-section training-source-section",
                header { class: "training-form-heading",
                    h3 { "Training data" }
                }
                Field { label: "Dataset", class: "training-field-wide",
                    Select {
                        value: dataset_id.clone(),
                        disabled: active_run.is_some(),
                        onchange: move |value: String| {
                            controller.edit_config(move |config| config.set_dataset_id(value));
                        },
                        for id in dataset_ids.iter() {
                            // Stored bare, and named the way every other
                            // activity names it.
                            option { value: "{id}", title: "dataset_{id}", "{id}" }
                        }
                    }
                }
                if let Some(problem) = catalog.problem() {
                    div { class: "training-inline-state training-inline-state-caution", role: "status",
                        div { class: "training-inline-copy",
                            strong { "Could not load datasets" }
                            span { "{problem}" }
                            p { "{problem.recovery_guidance()}" }
                            Button {
                                purpose: ActionPurpose::Retry,
                                variant: ButtonVariant::Outline,
                                aria_label: "Retry loading datasets",
                                onclick: move |_| catalog.reread(),
                                "Retry"
                            }
                        }
                    }
                } else if catalog.reading() {
                    p { class: "text-[11px] text-ink-faint", role: "status", "Loading datasets…" }
                }
                FactList { mono: true, class: "training-source-meta",
                    Fact { label: "Cameras", "{camera_summary}" }
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
                Field { label: "Device", class: "training-field-wide",
                    Select {
                        value: config.device().to_owned(),
                        disabled: active_run.is_some() || readiness.runtime().is_none(),
                        onchange: move |value: String| {
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
                    Field { label: "Run name",
                        TextInput {
                            value: config.run_name().to_owned(),
                            disabled: active_run.is_some(),
                            oninput: move |value| {
                                controller.set_run_name(value);
                            },
                        }
                    }
                    Field { label: "Steps",
                        TextInput {
                            kind: TextInputKind::Number, min: "1", step: "100",
                            value: "{config.steps()}",
                            disabled: active_run.is_some(),
                            oninput: move |value: String| if let Ok(value) = value.parse() {
                                controller.edit_config(move |config| config.set_steps(value));
                            },
                        }
                    }
                }
            }

            Disclosure { class: "training-query-section", summary: rsx! { "Advanced settings" },
                div { class: "training-query-body",
                    div { class: "training-form-grid",
                    Field { label: "Batch",
                        TextInput {
                            kind: TextInputKind::Number, min: "1",
                            value: "{config.batch_size()}",
                            disabled: active_run.is_some(),
                            oninput: move |value: String| if let Ok(value) = value.parse() {
                                controller.edit_config(move |config| config.set_batch_size(value));
                            },
                        }
                    }
                    Field { label: "Validation holdout (%)",
                        TextInput {
                            kind: TextInputKind::Number, min: "0", max: "50", step: "1",
                            title: "Whole episodes reserved for validation and excluded from optimizer updates",
                            value: "{validation_percent}",
                            disabled: active_run.is_some(),
                            oninput: move |value: String| if let Ok(value) = value.parse::<f64>() {
                                controller.edit_config(move |config| config.set_validation_split(value / 100.0));
                            },
                        }
                    }
                    Field { label: "Action horizon",
                        TextInput {
                            kind: TextInputKind::Number, min: "1", max: "256",
                            value: "{config.chunk_size()}",
                            disabled: active_run.is_some(),
                            oninput: move |value: String| if let Ok(value) = value.parse() {
                                controller.edit_config(move |config| config.set_chunk_size(value));
                            },
                        }
                    }
                    Field { label: "Learning rate",
                        TextInput {
                            kind: TextInputKind::Number, min: "0", step: "0.00001",
                            value: "{config.learning_rate()}",
                            disabled: active_run.is_some(),
                            oninput: move |value: String| if let Ok(value) = value.parse() {
                                controller.edit_config(move |config| config.set_learning_rate(value));
                            },
                        }
                    }
                    Field { label: "Checkpoint interval",
                        TextInput {
                            kind: TextInputKind::Number, min: "1", step: "100",
                            value: "{config.checkpoint_every()}",
                            disabled: active_run.is_some(),
                            oninput: move |value: String| if let Ok(value) = value.parse() {
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
                        Field { label: "State column",
                            TextInput {
                                value: config.state_column().to_owned(),
                                disabled: active_run.is_some(),
                                oninput: move |value: String| {
                                    controller.edit_config(move |config| config.set_state_column(value));
                                },
                            }
                        }
                        Field { label: "Action column",
                            TextInput {
                                value: config.action_column().to_owned(),
                                disabled: active_run.is_some(),
                                oninput: move |value: String| {
                                    controller.edit_config(move |config| config.set_action_column(value));
                                },
                            }
                        }
                    }
                    div { class: "training-column-row",
                        Field { label: "Camera streams", class: "training-field-wide",
                            TextInput {
                                value: config.act().camera_streams().join(", "),
                                placeholder: "All available",
                                disabled: active_run.is_some(),
                                oninput: move |value: String| {
                                    let streams = value
                                        .split(',')
                                        .map(str::trim)
                                        .filter(|stream| !stream.is_empty())
                                        .map(str::to_owned)
                                        .collect();
                                    controller.edit_config(move |config| config.act_mut().set_camera_streams(streams));
                                },
                            }
                        }
                        Field { label: "Image width (0 = source)",
                            TextInput {
                                kind: TextInputKind::Number, min: "0", max: "1024", step: "1",
                                value: "{config.act().image_width()}",
                                disabled: active_run.is_some(),
                                oninput: move |value: String| if let Ok(value) = value.parse() {
                                    controller.edit_config(move |config| config.act_mut().set_image_width(value));
                                },
                            }
                        }
                        Field { label: "Image height (0 = source)",
                            TextInput {
                                kind: TextInputKind::Number, min: "0", max: "1024", step: "1",
                                value: "{config.act().image_height()}",
                                disabled: active_run.is_some(),
                                oninput: move |value: String| if let Ok(value) = value.parse() {
                                    controller.edit_config(move |config| config.act_mut().set_image_height(value));
                                },
                            }
                        }
                    }
                }
            }
            }

            Button {
                purpose: ActionPurpose::Start,
                variant: ButtonVariant::Solid,
                size: ButtonSize::Prominent,
                class: "training-start-action",
                disabled: !can_start,
                aria_busy: starting,
                title: if active_run.is_some() { "Stop the active run first".to_owned() } else if let Some(reason) = training_blocker { reason.to_owned() } else if !dataset_valid { "Choose a valid dataset ID first".to_owned() } else if !readiness.ready() { readiness_recovery.clone() } else { "Start training".to_owned() },
                onclick: move |_| {
                    controller.start(project, training_history);
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
    checkpoints: Vec<TrainingCheckpoint>,
) -> Element {
    let confirming_delete = use_signal(|| None::<String>);
    rsx! {
        section { class: "training-history-panel studio-workbench-scroll",
            // Runs sits under Setup with no tab strip between them, so the
            // list names itself the way Setup's sections do.
            if !history.is_empty() {
                header { class: "training-form-heading training-history-heading", h3 { "Runs" } }
            }
            if history.is_empty() {
                EmptyState { title: "No runs yet",
                    p { "Every training run lands here with its curve and checkpoints." }
                }
            }
            for run in history {
                TrainingHistoryRow {
                    controller,
                    confirming_delete,
                    checkpoint_count: checkpoints.of_run(run.id()).len(),
                    run,
                }
            }
        }
    }
}

#[component]
fn TrainingHistoryRow(
    run: TrainingRun,
    checkpoint_count: usize,
    controller: super::state::TrainingController,
    mut confirming_delete: Signal<Option<String>>,
) -> Element {
    let studio = StudioProjectState::current();
    let selected = controller.selected_run().as_deref() == Some(run.id());
    let deleting_run = controller.deleting();
    let deleting = deleting_run.as_deref() == Some(run.id());
    let delete_disabled = run.phase().active() || deleting_run.is_some();
    let confirming = confirming_delete.read().as_deref() == Some(run.id());
    let checkpoint_word = if checkpoint_count == 1 {
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
            Button {
                purpose: ActionPurpose::Select,
                variant: ButtonVariant::Row,
                class: "training-history-row",
                aria_pressed: selected,
                title: "Open {run.config().run_name()}",
                onclick: {
                    let id = run.id().to_owned();
                    move |_| controller.open_run(id.clone())
                },
                span { class: "training-history-name", "{run.config().run_name()}" }
                span { class: "training-history-dataset", "{run.config().dataset_id()}" }
                span { class: run.phase_class(), "{run.phase().label()}" }
                span { class: "training-history-step", "{run.current_step()} / {run.config().steps()}" }
            }
            Button {
                id: "delete-run-{run.id()}",
                purpose: ActionPurpose::Delete,
                variant: ButtonVariant::Icon,
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
                            "Delete {} and its {} {}? They will be permanently removed.",
                            run.config().run_name(),
                            checkpoint_count,
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
fn TrainingRunDetail(
    run: TrainingRun,
    checkpoints: Vec<TrainingCheckpoint>,
    details_loaded: bool,
) -> Element {
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
                        span { class: run.phase_class(), "{run.phase().label()}" }
                    }
                    p { "Dataset · {run.config().dataset_id()} · {run.config().policy().label()} · {run.config().device()}" }
                }
                FactList { layout: FactLayout::Strip, mono: true, class: "training-run-facts",
                    Fact { label: "Step", "{run.current_step()} / {run.config().steps()}" }
                    Fact { label: "Progress", "{progress.clamp(0.0, 100.0):.1}%" }
                    Fact { label: "Checkpoints", "{checkpoints.len()}" }
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
                        TrainingLearning { run: run.clone(), checkpoints }
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
                Disclosure { class: "training-run-logs", summary: rsx! { "Log · {logs.len()} lines" },
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
fn TrainingLearning(run: TrainingRun, checkpoints: Vec<TrainingCheckpoint>) -> Element {
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
    let chart_id = run.chart_dom_id("training-loss");
    let chart_group = run.chart_group();
    let loss_chart = run.loss_chart();
    let ticks = run.event_ticks(&checkpoints);

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
                    for tick in ticks {
                        span {
                            class: "training-event-tick {tick.class}",
                            style: "left: {tick.percent:.3}%",
                            title: "{tick.label}",
                            "aria-label": "{tick.label}",
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
            FactList { layout: FactLayout::Tiles, mono: true, class: "training-input-facts",
                Fact { label: "Cameras", class: "training-input-wide", "{cameras}" }
                Fact { label: "Batch", "{run.config().batch_size()} samples" }
                Fact { label: "Validation", "{validation}" }
                Fact { label: "Action horizon", "{run.config().chunk_size()} steps" }
                Fact { label: "Image size", "{image_size}" }
                Fact { label: "Checkpoint interval", "{run.config().checkpoint_every()} steps" }
            }
        }
    }
}

#[component]
fn TrainingPerformance(run: TrainingRun) -> Element {
    let last = run.metrics().last();
    let chart_group = run.chart_group();
    let throughput = last
        .map(|point| format!("{:.1} samples/s", point.examples_per_second()))
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
        .map(ByteCountPresentation::byte_label)
        .unwrap_or_else(|| "—".to_owned());
    let loader_ms = last.and_then(|point| point.loader_ms()).millis_label();
    let device_copy_ms = last.and_then(|point| point.device_copy_ms()).millis_label();
    let model_ms = last.and_then(|point| point.model_ms()).millis_label();
    let throughput_chart = ResourceSeries::Throughput.chart(&run);
    let cpu_chart = ResourceSeries::Cpu.chart(&run);
    let gpu_chart = ResourceSeries::Gpu.chart(&run);
    let memory_chart = ResourceSeries::Memory.chart(&run);

    rsx! {
        section { class: "training-resources",
            FactList { layout: FactLayout::Strip, mono: true, class: "training-phase-timings", aria_label: "Latest optimizer step timing",
                Fact { label: "Load", title: "Spiral query, frame decode, normalization, and batch assembly. Prefetch overlaps this work with the preceding optimizer step.", "{loader_ms}" }
                Fact { label: "Device copy", title: "Runtime tensor conversion and host-to-device transfer.", "{device_copy_ms}" }
                Fact { label: "Model", title: "Runtime forward pass, backward pass, gradient clipping, and optimizer step.", "{model_ms}" }
            }
            div { class: "training-resource-grid",
                ResourceLane { lane: ResourceLaneModel::new(run.chart_dom_id("training-rate"), "Training rate", throughput).with_chart(throughput_chart, chart_group.clone()).with_presentation("resource-throughput", "Samples completed per second.") }
                ResourceLane { lane: ResourceLaneModel::new(run.chart_dom_id("cpu-utilization"), "CPU", cpu).with_chart(cpu_chart, chart_group.clone()).with_presentation("resource-cpu", "Runtime process CPU utilization. 100% is one fully occupied core.") }
                ResourceLane { lane: ResourceLaneModel::new(run.chart_dom_id("gpu-utilization"), "GPU", gpu).with_chart(gpu_chart, chart_group.clone()).with_presentation("resource-gpu", "Whole-device utilization reported by the selected runtime; other processes may contribute.") }
                ResourceLane { lane: ResourceLaneModel::new(run.chart_dom_id("memory-usage"), "Memory", memory).with_chart(memory_chart, chart_group).with_presentation("resource-memory", "Current resident memory used by the runtime process, including training allocations.") }
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
    Cpu,
    Gpu,
    Memory,
}

/// One run, as Training draws it: what its panel and charts are called, what
/// its phase looks like, and the series and marks read off its metrics.
/// `TrainingRun` lives in `crates/common`, so this is an extension trait rather
/// than an inherent impl; it is one trait for one subject, not one per method.
trait TrainingPresentation {
    fn panel_id(&self) -> String;
    fn chart_dom_id(&self, prefix: &str) -> String;
    /// Every chart of one run shares a zoom group, so dragging one step window
    /// moves them all.
    fn chart_group(&self) -> String;
    fn phase_class(&self) -> &'static str;
    /// The last step any evidence mentions, which is what the step axis spans.
    fn observed_steps(&self) -> u64;
    fn metric_series(&self, value: impl Fn(&TrainingMetricPoint) -> Option<f64>) -> DataFrame;
    fn loss_chart(&self) -> Chart;
    /// This run's checkpoints and metric events on one axis, already placed
    /// and labelled.
    fn event_ticks(&self, checkpoints: &[TrainingCheckpoint]) -> Vec<TrainingEventTick>;
}

/// One mark on the run's event strip.
struct TrainingEventTick {
    class: &'static str,
    percent: f64,
    label: String,
}

impl TrainingPresentation for TrainingRun {
    fn panel_id(&self) -> String {
        format!("training-run-{}", self.id())
    }

    fn chart_dom_id(&self, prefix: &str) -> String {
        let safe = self
            .id()
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

    fn chart_group(&self) -> String {
        self.chart_dom_id("training-step-window")
    }

    fn phase_class(&self) -> &'static str {
        match self.phase() {
            TrainingRunPhase::Starting | TrainingRunPhase::Running | TrainingRunPhase::Stopping => {
                "training-phase training-phase-active"
            }
            TrainingRunPhase::Completed => "training-phase training-phase-complete",
            TrainingRunPhase::Stopped | TrainingRunPhase::Unavailable => "training-phase",
            TrainingRunPhase::Failed => "training-phase training-phase-failed",
        }
    }

    fn observed_steps(&self) -> u64 {
        self.metrics()
            .last()
            .map_or(self.current_step(), |point| {
                point.step().max(self.current_step())
            })
            .max(1)
    }

    fn metric_series(&self, value: impl Fn(&TrainingMetricPoint) -> Option<f64>) -> DataFrame {
        let points = self
            .metrics()
            .iter()
            .filter_map(|point| {
                let value = value(point)?;
                (value.is_finite() && value >= 0.0).then_some((point.step() as f64, value))
            })
            .collect::<Vec<_>>();
        points
            .downsample_extrema(TRAINING_CHART_POINT_BUDGET)
            .into_iter()
            .map(|(step, value)| TrainingChartStyle::point(step, Some(value)))
            .collect()
    }

    fn loss_chart(&self) -> Chart {
        let train = self.metric_series(|point| Some(point.train_loss()).filter(|loss| *loss > 0.0));
        let validation =
            self.metric_series(|point| point.validation_loss().filter(|loss| *loss > 0.0));
        let (train_color, validation_color) =
            (ChartColor::Train.css(), ChartColor::Validation.css());

        Chart::new()
            .animation(false)
            .color(vec![train_color.clone(), validation_color.clone()])
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
                    .max(self.observed_steps() as f64)
                    .split_number(4),
            )
            .y_axis(TrainingChartStyle::axis(AxisType::Value, true).split_number(5))
            .data_zoom(TrainingChartStyle::slider_zoom(48.0, 12.0))
            .series(
                MetricLineSpec::new("train-loss", "Train", train_color)
                    .with_width(2.0)
                    .render(train),
            )
            .series(
                MetricLineSpec::new("validation-loss", "Validation", validation_color)
                    .with_width(1.8)
                    .render(validation),
            )
    }

    fn event_ticks(&self, checkpoints: &[TrainingCheckpoint]) -> Vec<TrainingEventTick> {
        let place = |step: u64| {
            (step as f64 / self.config().steps().max(1) as f64 * 100.0).clamp(0.0, 100.0)
        };
        checkpoints
            .iter()
            .map(|checkpoint| TrainingEventTick {
                class: "training-event-checkpoint",
                percent: place(checkpoint.step()),
                label: format!("Checkpoint saved at step {}", checkpoint.step()),
            })
            .chain(self.metrics().iter().filter_map(|point| {
                let event = point.event()?;
                Some(TrainingEventTick {
                    class: if event.contains("loader stall") {
                        "training-event-stall"
                    } else {
                        "training-event-validation"
                    },
                    percent: place(point.step()),
                    label: format!("{event} at step {}", point.step()),
                })
            }))
            .collect()
    }
}

impl ResourceSeries {
    fn chart(self, run: &TrainingRun) -> Chart {
        let (id, name, color) = match self {
            Self::Throughput => ("training-rate", "Training rate", ChartColor::Throughput),
            Self::Cpu => ("cpu-utilization", "CPU", ChartColor::Cpu),
            Self::Gpu => ("gpu-utilization", "GPU", ChartColor::Gpu),
            Self::Memory => ("memory-usage", "Memory", ChartColor::Memory),
        };
        let color = color.css();
        let data = run.metric_series(|point| match self {
            Self::Throughput => Some(point.examples_per_second()),
            Self::Cpu => point.cpu_percent(),
            Self::Gpu => point.gpu_percent(),
            Self::Memory => point.memory_bytes().map(|value| value as f64),
        });
        let tooltip = match self {
            Self::Throughput => StudioTooltip::samples_per_second(),
            Self::Cpu | Self::Gpu => StudioTooltip::percent(),
            Self::Memory => StudioTooltip::bytes(),
        };

        Chart::new()
            .animation(false)
            .color(vec![color.clone()])
            .tooltip(tooltip)
            .grid(Grid::new().left(0).right(0).top(6).bottom(40))
            .x_axis(
                TrainingChartStyle::axis(AxisType::Value, false)
                    .min(0)
                    .max(run.observed_steps() as f64),
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
                MetricLineSpec::new(id, name, color.clone())
                    .with_width(1.5)
                    .render(data)
                    .area_style(AreaStyle::new().color(color).opacity(0.09)),
            )
    }
}

struct MetricLineSpec {
    id: &'static str,
    name: &'static str,
    color: String,
    width: f64,
}

impl MetricLineSpec {
    fn new(id: &'static str, name: &'static str, color: String) -> Self {
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
            .background_color(ChartColor::Surface.css())
            .border_color(ChartColor::Line.css())
            .filler_color(ChartColor::Selected.css())
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
                    .color(ChartColor::InkFaint.css()),
            )
            .split_line(
                SplitLine::new()
                    .show(visible)
                    .line_style(LineStyle::new().color(ChartColor::Line.css()).width(1)),
            )
    }

    fn point(x: f64, y: Option<f64>) -> DataPoint {
        DataPoint::from(vec![CompositeValue::from(x), CompositeValue::from(y)])
    }
}

/// The checkpoints Spiral holds for one run, read off every run's checkpoints.
trait RunCheckpoints {
    fn of_run(&self, run_id: &str) -> Vec<TrainingCheckpoint>;
}

impl RunCheckpoints for [TrainingCheckpoint] {
    fn of_run(&self, run_id: &str) -> Vec<TrainingCheckpoint> {
        self.iter()
            .filter(|checkpoint| checkpoint.run_id() == run_id)
            .cloned()
            .collect()
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

    use super::*;
    use spiral_robotics_common::training::{
        TrainingConfig, TrainingRunHistory, TrainingRunIdentity, TrainingRunStatus,
    };

    fn run_with(status: TrainingRunStatus, metrics: Vec<TrainingMetricPoint>) -> TrainingRun {
        TrainingRun::new(
            TrainingRunIdentity::new("run", TrainingConfig::default(), 1),
            status,
        )
        .with_history(TrainingRunHistory::new(metrics, Vec::new()))
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
        let run = run_with(
            TrainingRunStatus::new(TrainingRunPhase::Running, 100, None),
            vec![TrainingMetricPoint::new(50, 0.0).with_examples_per_second(25.0)],
        );
        let option = serde_json::to_value(ResourceSeries::Throughput.chart(&run))
            .expect("chart should serialize");
        assert_eq!(option["xAxis"]["max"], 100.0);
        assert_eq!(option["series"][0]["data"][0][0], 50.0);
        assert_eq!(option["dataZoom"].as_array().map(Vec::len), Some(1));
        assert_eq!(option["dataZoom"][0]["type"], "slider");
        assert_eq!(option["dataZoom"][0]["show"], true);
        assert_eq!(option["dataZoom"][0]["xAxisIndex"], 0);
    }

    /// Checkpoints and metric events land on one axis, as a fraction of the
    /// steps the run was configured for.
    #[wasm_bindgen_test::wasm_bindgen_test]
    fn event_ticks_place_checkpoints_and_stalls_on_the_configured_step_axis() {
        let run = run_with(
            TrainingRunStatus::default(),
            vec![TrainingMetricPoint::new(25, 0.5).with_event("loader stall")],
        );
        let ticks = run.event_ticks(&[TrainingCheckpoint::new("run", 50, 0)]);
        assert_eq!(ticks[0].class, "training-event-checkpoint");
        assert_eq!(ticks[0].label, "Checkpoint saved at step 50");
        assert_eq!(ticks[1].class, "training-event-stall");
        assert_eq!(ticks[1].label, "loader stall at step 25");
        assert!(ticks[0].percent > ticks[1].percent);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn loss_chart_uses_a_linear_axis_and_slider_zoom_with_stable_series() {
        let run = run_with(
            TrainingRunStatus::default(),
            vec![TrainingMetricPoint::new(1, 0.25).with_validation_loss(Some(0.3))],
        );
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
    let (workspace, project) = crate::shell::activity::WorkspaceActivity::mount(
        crate::shell::activity::Activity::Training,
        is_active,
    );
    let controller = super::state::TrainingController::use_controller();
    let history =
        super::history::TrainingHistory::use_history(is_active, move || controller.selected_run());
    rsx! { crate::shell::frame::WorkspaceFrame { workspace, status: crate::shell::frame::WorkspaceStatus::Training(history), TrainingPanels { project, controller, training_history: history } } }
}
