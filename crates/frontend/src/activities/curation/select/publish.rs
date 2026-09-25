//! Publishing an output: its path compiled over the whole dataset and created
//! as a catalog view under the name the operator types, then recorded in the
//! curation, which saves. A view cannot be turned back into a graph, so the
//! recipe stays in the curation and the view is what it produces.
use crate::activities::curation::path::{CurationPath, PathSource};
use crate::activities::curation::state::{CurationController, RunState};
use crate::client::project::ProjectClient;
use crate::client::queries::QueryDomain;
use crate::client::transport::HttpRequest;
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use spiral_robotics_common::curation::{PublishRequest, UnpublishRequest, ViewName};
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant, TextInput};

/// Where the control stands between presses.
#[derive(Clone, Debug, PartialEq)]
enum PublishStep {
    Resting,
    /// The curation has unsaved edits. The view is recorded against the
    /// saved recipe, so they are saved first.
    SavingFirst,
    /// The operator is typing the view's name.
    Naming(String),
}

/// One output's view, created or dropped through the backend and recorded in
/// the curation. Each runs as the curation's own task, so choosing another
/// node never leaves a view created and unrecorded.
#[derive(Clone, PartialEq)]
struct ViewPublisher {
    controller: CurationController,
    studio: StudioProjectState,
    node: String,
}

impl ViewPublisher {
    /// Publish the saved recipe's `statement` as the view `name`.
    fn publish(&self, name: String, statement: String) {
        let (controller, studio) = (self.controller, self.studio);
        let saved = controller.saved_revision();
        controller.start_run(&self.node, move |run| async move {
            run.report(format!("Publishing as {name}"));
            let request = HttpRequest::new("POST", "/api/curations/publish")
                .json(&PublishRequest::new(name.clone(), statement));
            let recorded = ProjectClient::current()
                .mutate::<()>(&[QueryDomain::Catalog], request)
                .await
                .map_err(|error| error.to_string())
                .and_then(|()| {
                    controller.edit(|payload| payload.record_publication(run.node(), &name, saved))
                });
            match recorded {
                Ok(()) => {
                    controller.save_when_idle(studio).await;
                    studio
                        .report_success_from(MessageSource::Curation, format!("Published {name}"));
                }
                Err(error) => studio.report_from(MessageSource::Curation, error),
            }
        });
    }

    /// Drop the view `name` and forget that the output is published.
    fn unpublish(&self, name: String) {
        let (controller, studio) = (self.controller, self.studio);
        let saved = controller.saved_revision();
        controller.start_run(&self.node, move |run| async move {
            run.report(format!("Unpublishing {name}"));
            let request = HttpRequest::new("POST", "/api/curations/unpublish")
                .json(&UnpublishRequest::new(name.clone()));
            let forgotten = ProjectClient::current()
                .mutate::<()>(&[QueryDomain::Catalog], request)
                .await
                .map_err(|error| error.to_string())
                .and_then(|()| {
                    controller.edit(|payload| payload.remove_publication(run.node(), saved))
                });
            match forgotten {
                Ok(()) => controller.save_when_idle(studio).await,
                Err(error) => studio.report_from(MessageSource::Curation, error),
            }
        });
    }
}

/// Publish, the name it is published under, and whether the saved recipe has
/// moved on since. An output whose path runs through a node Spiral has no
/// function for yet says which function it waits for instead.
#[component]
pub(super) fn PublishControl(controller: CurationController, node: String) -> Element {
    let studio = StudioProjectState::current();
    let mut step = use_signal(|| PublishStep::Resting);
    let statement = controller.with_payload(|payload| {
        CurationPath::to(payload, &node).and_then(|path| path.statement(PathSource::Published))
    });
    let statement = match statement {
        Ok(statement) => statement,
        Err(waiting) => {
            return rsx! {
                p { class: "curation-select-publish-note", role: "status", "{waiting.message()}" }
            };
        }
    };
    let publication = controller.with_payload(|payload| payload.publication(&node).cloned());
    let saved = controller.saved_revision();
    let unsaved = controller.unsaved_changes();
    let busy = controller.run_state(&node) != RunState::Idle || controller.command().is_some();
    let publisher = ViewPublisher {
        controller,
        studio,
        node: node.clone(),
    };
    let named = publication
        .as_ref()
        .map(|published| published.view().to_owned())
        .unwrap_or_default();
    rsx! {
        div { class: "curation-select-publish",
            match step() {
                PublishStep::Resting => rsx! {
                    if let Some(published) = publication.clone() {
                        if published.is_current(saved) {
                            p { class: "curation-select-publish-status", role: "status",
                                title: "The catalog view {published.view()} is what the saved curation selects",
                                "Published as "
                                code { "{published.view()}" }
                            }
                        } else {
                            p { class: "curation-select-publish-status is-stale", role: "status",
                                title: "The saved curation has changed since {published.view()} was published; publish again to update the view",
                                code { "{published.view()}" }
                                " is out of date"
                            }
                        }
                        Button {
                            purpose: ActionPurpose::Remove, variant: ButtonVariant::Outline,
                            disabled: busy || unsaved,
                            title: if unsaved { "Save the curation before unpublishing" } else { "Drop the catalog view" },
                            onclick: {
                                let publisher = publisher.clone();
                                let view = published.view().to_owned();
                                move |_| publisher.unpublish(view.clone())
                            },
                            "Unpublish"
                        }
                    }
                    Button {
                        purpose: ActionPurpose::Export, variant: ButtonVariant::Solid,
                        disabled: busy,
                        title: "Create a catalog view of what this Select keeps, over the whole dataset",
                        onclick: {
                            let named = named.clone();
                            move |_| step.set(if unsaved { PublishStep::SavingFirst } else { PublishStep::Naming(named.clone()) })
                        },
                        if publication.is_some() { "Publish again" } else { "Publish" }
                    }
                },
                PublishStep::SavingFirst => rsx! {
                    p { class: "curation-select-publish-status", role: "status", "Save the curation first: the view is recorded in it." }
                    Button {
                        purpose: ActionPurpose::Cancel, variant: ButtonVariant::Link,
                        onclick: move |_| step.set(PublishStep::Resting),
                        "Cancel"
                    }
                    Button {
                        purpose: ActionPurpose::Apply, variant: ButtonVariant::Solid,
                        disabled: controller.command().is_some(),
                        onclick: {
                            let named = named.clone();
                            move |_| {
                                controller.save(studio);
                                step.set(PublishStep::Naming(named.clone()));
                            }
                        },
                        "Save"
                    }
                },
                PublishStep::Naming(name) => {
                    let problem = (!name.is_empty())
                        .then(|| ViewName::try_from(name.as_str()).err())
                        .flatten();
                    let ready = !name.is_empty() && problem.is_none() && !unsaved && !busy;
                    rsx! {
                        TextInput {
                            class: "curation-select-publish-name",
                            value: name.clone(),
                            placeholder: "view_name",
                            autofocus: true,
                            "aria-label": "Name of the catalog view",
                            "aria-invalid": problem.is_some().then_some("true"),
                            oninput: move |text| step.set(PublishStep::Naming(text)),
                        }
                        if let Some(problem) = problem {
                            p { class: "curation-select-publish-problem", role: "alert", "{problem}" }
                        }
                        Button {
                            purpose: ActionPurpose::Cancel, variant: ButtonVariant::Link,
                            onclick: move |_| step.set(PublishStep::Resting),
                            "Cancel"
                        }
                        Button {
                            purpose: ActionPurpose::Export, variant: ButtonVariant::Solid,
                            disabled: !ready,
                            title: if unsaved { "Saving the curation…" } else { "Create or replace the view under this name" },
                            onclick: {
                                let publisher = publisher.clone();
                                let statement = statement.clone();
                                move |_| {
                                    publisher.publish(name.clone(), statement.clone());
                                    step.set(PublishStep::Resting);
                                }
                            },
                            "Publish"
                        }
                    }
                }
            }
        }
    }
}
