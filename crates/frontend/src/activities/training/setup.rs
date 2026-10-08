//! The Setup panel: which model the next run trains, what it trains on, how
//! that model sees the data, and how long it trains; and, while a run is
//! active, how far it has got and the way to stop it.

use dioxus::prelude::*;
use spiral_robotics_common::training::{
    TrainingModel, TrainingModelKind, TrainingRun, TrainingRunPhase,
};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, Disclosure, Field, FieldLayout, ModeButton,
    ModeGroup, Notice, NoticeTone, TextInput, TextInputKind,
};

use super::history::TrainingHistory;
use super::observation::{ActObservationSection, ProgressObservationSection};
use super::run_detail::TrainingPresentation;
use super::source::{TrainingDataReads, TrainingDataSection};
use super::state::TrainingController;
use crate::shell::state::StudioProjectState;

#[component]
pub(super) fn TrainingSetup(
    project: StudioProjectState,
    controller: TrainingController,
    training_history: TrainingHistory,
) -> Element {
    let reads = TrainingDataReads::use_reads(controller);
    let config = controller.config();
    let views = reads.views();
    let dataset_inputs = reads.inputs(config.source().dataset());
    let listed =
        matches!(&views, Some(Ok(views)) if views.source(config.source().view()).is_some());
    let observed = dataset_inputs
        .as_ref()
        .and_then(|read| read.as_ref().ok())
        .filter(|_| listed)
        .cloned();
    let snapshot = training_history.snapshot().read().to_owned();
    let readiness = snapshot.readiness().to_owned();
    let active_run = snapshot.active_run().cloned();
    let starting = controller.starting();
    let locked = active_run.is_some();
    let run_names = snapshot
        .runs()
        .iter()
        .map(|run| run.config().run_name().to_owned())
        .collect::<Vec<_>>();
    // Only a policy reads the joints.
    let policy = matches!(config.model(), TrainingModel::Act(_));
    let blocker = if active_run.is_some() {
        Some("Stop the active run first".to_owned())
    } else if !listed {
        Some("Choose a published view first".to_owned())
    } else if policy && observed.as_ref().is_none_or(|read| read.joints().is_err()) {
        Some("The dataset’s joints must be identified first".to_owned())
    } else if config.model().cameras().is_empty() {
        Some("Choose a camera first".to_owned())
    } else if !readiness.ready() {
        Some(readiness.summary().to_owned())
    } else {
        None
    };

    rsx! {
        div { class: "training-control",
            div { class: "training-control-scroll",
                if !readiness.ready() {
                    Notice { tone: NoticeTone::Caution, "{readiness.summary()}" }
                }
                if let Some(run) = active_run {
                    ActiveRun { project, controller, run }
                }
                ModelChoice { controller, run_names, locked }
                TrainingDataSection { controller, views, inputs: dataset_inputs, locked }
                if let Some(read) = observed {
                    match config.model() {
                        TrainingModel::Act(act) => rsx! {
                            ActObservationSection {
                                controller,
                                dataset: read.dataset().clone(),
                                cameras: read.cameras().clone(),
                                observation: act.observation().clone(),
                                locked,
                            }
                        },
                        TrainingModel::Progress(progress) => rsx! {
                            ProgressObservationSection {
                                controller,
                                cameras: read.cameras().clone(),
                                camera: progress.camera().to_owned(),
                                locked,
                            }
                        },
                    }
                }
                RunSection { controller, locked }
            }
            Button {
                purpose: ActionPurpose::Start,
                variant: ButtonVariant::Solid,
                size: ButtonSize::Prominent,
                class: "training-start-action",
                disabled: blocker.is_some() || starting,
                aria_busy: starting,
                title: blocker.unwrap_or_else(|| "Start training".to_owned()),
                onclick: move |_| controller.start(project, training_history),
                if starting { "Starting…" } else { "Run training" }
            }
        }
    }
}

/// The active run, how far it has got, and the way to stop it.
#[component]
fn ActiveRun(
    project: StudioProjectState,
    controller: TrainingController,
    run: TrainingRun,
) -> Element {
    let stopping = run.phase() == TrainingRunPhase::Stopping || controller.stopping().is_some();
    let id = run.id().to_owned();
    rsx! {
        section { class: "training-active-run",
            div { class: "training-active-heading",
                div {
                    strong { "{run.config().run_name()}" }
                    span { class: run.phase_class(), "{run.phase().label()}" }
                    small { "{run.config().source().view()}" }
                }
                if !run.phase().before_first_step() {
                    span { class: "training-step-readout", "{run.current_step()} / {run.config().steps()}" }
                }
            }
            div {
                class: "training-progress-track",
                role: "progressbar",
                "aria-label": "Training progress",
                "aria-valuemin": "0",
                "aria-valuemax": "{run.config().steps()}",
                "aria-valuenow": "{run.current_step()}",
                "aria-valuetext": "{run.progress_readout()}",
                div { class: "training-progress-fill", style: "width: {run.progress_percent():.2}%" }
            }
            // Before its first step a run's message is how far it has got;
            // while it runs, the step does.
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
                disabled: stopping,
                aria_busy: stopping,
                onclick: move |_| controller.stop(project, id.clone()),
                if stopping { "Stopping…" } else { "Stop training" }
            }
        }
    }
}

/// Which family of model the next run trains; choosing one takes its
/// family's defaults.
#[component]
fn ModelChoice(controller: TrainingController, run_names: Vec<String>, locked: bool) -> Element {
    let chosen = controller.config().model().kind();
    rsx! {
        section { class: "training-form-section",
            ModeGroup { label: "Model".to_owned(), class: "training-model-choice".to_owned(),
                for kind in TrainingModelKind::ALL {
                    ModeButton {
                        key: "{kind.label()}",
                        purpose: ActionPurpose::Select,
                        checked: kind == chosen,
                        disabled: locked,
                        onclick: {
                            let run_names = run_names.clone();
                            move |_| controller.choose_model(kind, &run_names)
                        },
                        "{kind.label()}"
                    }
                }
            }
        }
    }
}

/// The run's name and length, and the optimizer's settings on demand.
#[component]
fn RunSection(controller: TrainingController, locked: bool) -> Element {
    let config = controller.config();
    let validation_percent = config.validation_split() * 100.0;
    rsx! {
        section { class: "training-form-section",
            header { class: "training-form-heading", h3 { "Run" } }
            Field { label: "Run name", layout: FieldLayout::Inline,
                TextInput {
                    value: config.run_name().to_owned(),
                    disabled: locked,
                    oninput: move |value| controller.set_run_name(value),
                }
            }
            Field { label: "Steps", layout: FieldLayout::Inline,
                TextInput {
                    kind: TextInputKind::Number, min: "1", step: "100",
                    value: "{config.steps()}",
                    disabled: locked,
                    oninput: move |value: String| if let Ok(value) = value.parse() {
                        controller.edit_config(move |config| config.set_steps(value));
                    },
                }
            }
        }
        Disclosure { class: "training-advanced", summary: rsx! { "Advanced settings" },
            div { class: "training-form-grid",
                Field { label: "Batch",
                    TextInput {
                        kind: TextInputKind::Number, min: "1",
                        value: "{config.batch_size()}",
                        disabled: locked,
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
                        disabled: locked,
                        oninput: move |value: String| if let Ok(value) = value.parse::<f64>() {
                            controller.edit_config(move |config| config.set_validation_split(value / 100.0));
                        },
                    }
                }
                if let TrainingModel::Act(act) = config.model() {
                    Field { label: "Action horizon",
                        TextInput {
                            kind: TextInputKind::Number, min: "1", max: "256",
                            value: "{act.chunk_size()}",
                            disabled: locked,
                            oninput: move |value: String| if let Ok(value) = value.parse() {
                                controller.edit_act(move |act| act.set_chunk_size(value));
                            },
                        }
                    }
                }
                Field { label: "Learning rate",
                    TextInput {
                        kind: TextInputKind::Number, min: "0", step: "0.00001",
                        value: "{config.learning_rate()}",
                        disabled: locked,
                        oninput: move |value: String| if let Ok(value) = value.parse() {
                            controller.edit_config(move |config| config.set_learning_rate(value));
                        },
                    }
                }
                Field { label: "Checkpoint interval",
                    TextInput {
                        kind: TextInputKind::Number, min: "1", step: "100",
                        value: "{config.checkpoint_every()}",
                        disabled: locked,
                        oninput: move |value: String| if let Ok(value) = value.parse() {
                            controller.edit_config(move |config| config.set_checkpoint_every(value));
                        },
                    }
                }
            }
        }
    }
}
