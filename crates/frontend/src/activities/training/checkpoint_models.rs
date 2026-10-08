//! A progress run's checkpoints as models Score progress can score with:
//! each checkpoint the run saved, with the name it was published as, or a
//! name field and Publish. A policy's checkpoints go to Rollout instead.

use dioxus::prelude::*;
use spiral_robotics_common::training::{ModelPublication, PublishedModel, TrainingCheckpoint};
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant, EmptyState, TextInput};

use super::TrainingClient;
use super::state::TrainingController;
use crate::client::CommandError;
use crate::client::queries::{QueryDomain, QueryPolicy};
use crate::format::LossPresentation;
use crate::shell::project::ProjectSelection;
use crate::shell::state::StudioProjectState;

/// The progress models the project published, read when Training opens and
/// again after each Training command, publishing included.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct PublishedModels(Resource<Result<Vec<PublishedModel>, CommandError>>);

impl PublishedModels {
    pub(super) fn use_models(is_active: ReadSignal<bool>) -> Self {
        let selection = ProjectSelection::current();
        Self(QueryPolicy::on_demand(QueryDomain::Training).use_when(
            move || is_active() && selection.live(),
            || async { TrainingClient::current().published_models().await },
        ))
    }

    /// Those read so far: none before the first read answers.
    pub(super) fn listed(self) -> Vec<PublishedModel> {
        match &*self.0.read() {
            Some(Ok(models)) => models.clone(),
            _ => Vec::new(),
        }
    }
}

/// A progress run's checkpoints, latest first, each with what it was
/// published as among `published`.
#[component]
pub(super) fn CheckpointModels(
    controller: TrainingController,
    checkpoints: Vec<TrainingCheckpoint>,
    published: Vec<PublishedModel>,
) -> Element {
    if checkpoints.is_empty() {
        return rsx! { EmptyState { title: "No checkpoints yet" } };
    }
    rsx! {
        ol { class: "training-checkpoint-models",
            for checkpoint in checkpoints.iter().rev() {
                CheckpointModel {
                    key: "{checkpoint.step()}",
                    controller,
                    names: published
                        .iter()
                        .filter(|model| {
                            model.run_id() == checkpoint.run_id() && model.step() == checkpoint.step()
                        })
                        .map(|model| model.name().to_owned())
                        .collect::<Vec<_>>(),
                    checkpoint: checkpoint.clone(),
                }
            }
        }
    }
}

/// One checkpoint: its step and validation loss, then the names it was
/// published as, or a name field and Publish.
#[component]
fn CheckpointModel(
    controller: TrainingController,
    checkpoint: TrainingCheckpoint,
    names: Vec<String>,
) -> Element {
    let studio = StudioProjectState::current();
    let mut name = use_signal(String::new);
    let publishing = controller.publishing();
    let step = checkpoint.step();
    let this_one = publishing
        .as_ref()
        .is_some_and(|(run, publishing)| run == checkpoint.run_id() && *publishing == step);
    let loss = checkpoint
        .validation_loss()
        .map(LossPresentation::loss_label);
    let run_id = checkpoint.run_id().to_owned();
    rsx! {
        li { class: "training-checkpoint-model",
            span { class: "training-checkpoint-step", "Step {step}" }
            if let Some(loss) = loss {
                span { class: "training-checkpoint-loss", title: "Validation loss", "{loss}" }
            }
            if names.is_empty() {
                form {
                    class: "training-checkpoint-publish",
                    onsubmit: move |event: FormEvent| {
                        event.prevent_default();
                        let typed = name.peek().trim().to_owned();
                        if !typed.is_empty() {
                            controller.publish(studio, ModelPublication::new(run_id.clone(), step, typed));
                        }
                    },
                    TextInput {
                        class: "training-checkpoint-name",
                        value: name(),
                        placeholder: "Model name",
                        disabled: this_one,
                        "aria-label": "Name to publish step {step} as",
                        oninput: move |value| name.set(value),
                    }
                    Button {
                        purpose: ActionPurpose::Export,
                        variant: ButtonVariant::Outline,
                        r#type: "submit",
                        disabled: name().trim().is_empty() || publishing.is_some(),
                        title: "Publish this checkpoint as a model Score progress can score with",
                        if this_one { "Publishing…" } else { "Publish" }
                    }
                }
            } else {
                span { class: "training-checkpoint-published", "{names.join(\" · \")}" }
            }
        }
    }
}
