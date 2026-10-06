//! The Training screen: Setup and Runs down the left, the opened runs beside
//! them.

use crate::components::icons::TrainingIcon;
use dioxus::prelude::*;
use dioxus_workbench::{LayoutNode, Panel, PanelId, PanelLayout, PanelWorkspace, SplitAxis};
use spiral_robotics_common::training::{TrainingCheckpoint, TrainingRun};
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, ConfirmationCopy, ConfirmationDismissal,
    ConfirmationResponse, DeleteIcon, EmptyState, InlineConfirmation,
};

use super::run_detail::{RunCheckpoints, TrainingPresentation, TrainingRunDetail};
use super::setup::TrainingSetup;
use crate::shell::state::StudioProjectState;

const EMPTY_RUN_PANEL_ID: &str = "training-run-empty";

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
            TrainingSetup { project, controller, training_history }
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
                    EmptyState { icon: rsx! { TrainingIcon {} }, title: "No run open" }
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
                EmptyState { title: "No runs yet" }
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
                span { class: "training-history-view", "{run.config().source().view()}" }
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
pub(crate) fn TrainingWorkspace(is_active: ReadSignal<bool>) -> Element {
    let workspace = crate::shell::activity::WorkspaceActivity::provide(
        crate::shell::activity::Activity::Training,
        is_active,
    );
    let project = crate::shell::state::StudioProjectState::current();
    let controller = super::state::TrainingController::use_controller();
    let history =
        super::history::TrainingHistory::use_history(is_active, move || controller.selected_run());
    rsx! { crate::shell::frame::WorkspaceFrame { workspace, status: crate::shell::frame::WorkspaceStatus::Training(history), TrainingPanels { project, controller, training_history: history } } }
}
