//! Publishing from the document bar: the outputs the operator chooses, each
//! compiled over the whole dataset and created as a catalog view under the
//! name they give it, then recorded in the curation, which saves. A view
//! cannot be turned back into a graph, so the recipe stays in the curation
//! and the view is what it produces.
//!
//! A publication is recorded against the saved recipe, and the save that
//! records it must come next. So one press creates every chosen view, records
//! them all in one edit, and saves once; and nothing else publishes or
//! unpublishes until that is done.
use super::node_card::NodeCard;
use crate::activities::curation::document::PublishedOutput;
use crate::activities::curation::path::CurationPath;
use crate::activities::curation::state::CurationController;
use crate::client::project::ProjectClient;
use crate::client::queries::QueryDomain;
use crate::client::transport::HttpRequest;
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use spiral_robotics_common::curation::{PublishRequest, UnpublishRequest, ViewName};
use spiral_robotics_ui::{
    ActionPurpose, BrowserFocus, Button, ButtonVariant, ChevronIcon, TextInput, Toggle,
};
use std::collections::{BTreeMap, BTreeSet};

/// Focus has to be inside the panel for its own Escape handler to hear the
/// key, the way the document switcher's is.
const PUBLISH_DIALOG_ID: &str = "curation-publish-dialog";

/// One output as the panel lists it: its node, how its card reads, the
/// statement its view would be, or why it has none, and the view it is
/// published as.
#[derive(Clone, Debug, PartialEq)]
struct PublishableOutput {
    node: String,
    label: &'static str,
    summary: String,
    statement: Result<String, String>,
    published: Option<PublishedOutput>,
}

impl PublishableOutput {
    /// Every output of the open curation, in graph order.
    fn all(controller: CurationController) -> Vec<Self> {
        controller.with_reading(|payload, reader| {
            let cards = NodeCard::all(payload, reader);
            payload
                .outputs()
                .into_iter()
                .filter_map(|output| {
                    let card = cards.iter().find(|card| card.id() == output.id())?;
                    Some(Self {
                        node: output.id().to_owned(),
                        label: card.label(),
                        summary: card.summary().to_owned(),
                        statement: CurationPath::to(payload, output.id())
                            .and_then(|path| path.statement())
                            .map_err(|error| error.message().to_owned()),
                        published: payload.publication(output.id()).cloned(),
                    })
                })
                .collect()
        })
    }
}

/// What the operator has chosen while the panel is open: the name typed for
/// each output, starting from the view it is published as, and which outputs
/// to publish.
#[derive(Clone, Debug, Default, PartialEq)]
struct PublishChoice {
    names: BTreeMap<String, String>,
    chosen: BTreeSet<String>,
}

impl PublishChoice {
    fn of(outputs: &[PublishableOutput]) -> Self {
        Self {
            names: outputs
                .iter()
                .map(|output| {
                    let name = output.published.as_ref().map(PublishedOutput::view);
                    (output.node.clone(), name.unwrap_or_default().to_owned())
                })
                .collect(),
            chosen: BTreeSet::new(),
        }
    }
    fn name(&self, node: &str) -> &str {
        self.names.get(node).map_or("", String::as_str)
    }
    /// Why `output`, chosen, cannot be published under the name typed for
    /// it: no name, not a view name, a name another chosen output or another
    /// output's view already has.
    fn problem(&self, output: &PublishableOutput, outputs: &[PublishableOutput]) -> Option<String> {
        let name = self.name(&output.node);
        if name.is_empty() {
            return Some("Name the view.".to_owned());
        }
        if let Err(problem) = ViewName::try_from(name) {
            return Some(problem);
        }
        let taken = outputs
            .iter()
            .filter(|other| other.node != output.node)
            .any(|other| {
                (self.chosen.contains(&other.node) && self.name(&other.node) == name)
                    || other
                        .published
                        .as_ref()
                        .is_some_and(|published| published.view() == name)
            });
        taken.then(|| format!("Another output is named “{name}”."))
    }
    /// The chosen outputs as views to create, once every one of them can be.
    fn views(&self, outputs: &[PublishableOutput]) -> Option<Vec<(String, String, String)>> {
        let chosen = outputs
            .iter()
            .filter(|output| self.chosen.contains(&output.node))
            .collect::<Vec<_>>();
        if chosen.is_empty() {
            return None;
        }
        chosen
            .into_iter()
            .map(|output| {
                if self.problem(output, outputs).is_some() {
                    return None;
                }
                let statement = output.statement.clone().ok()?;
                Some((
                    output.node.clone(),
                    self.name(&output.node).to_owned(),
                    statement,
                ))
            })
            .collect()
    }
}

/// Creates and drops the curation's views through the backend and records
/// them in it, one operation at a time.
#[derive(Clone, Copy, PartialEq)]
struct ViewPublisher {
    controller: CurationController,
    studio: StudioProjectState,
    busy: Signal<bool>,
}

impl ViewPublisher {
    /// Create each `(node, view, statement)` in turn, then record every view
    /// that was created in one edit of the recipe saved now, and save.
    fn publish(self, views: Vec<(String, String, String)>) {
        let Self {
            controller,
            studio,
            mut busy,
        } = self;
        let saved = controller.saved_revision();
        busy.set(true);
        spawn(async move {
            let mut created = Vec::new();
            for (node, view, statement) in views {
                controller.report_activity(&node, Some(format!("Publishing as {view}")));
                let request = HttpRequest::new("POST", "/api/curations/publish")
                    .json(&PublishRequest::new(view.clone(), statement));
                let outcome = ProjectClient::current()
                    .mutate::<()>(&[QueryDomain::Catalog], request)
                    .await;
                controller.report_activity(&node, None);
                match outcome {
                    Ok(()) => created.push((node, view)),
                    Err(error) => studio.report_from(MessageSource::Curation, error.to_string()),
                }
            }
            if !created.is_empty() {
                let recorded = controller.edit(|payload| {
                    created
                        .iter()
                        .try_for_each(|(node, view)| payload.record_publication(node, view, saved))
                });
                match recorded {
                    Ok(()) => {
                        controller.save_when_idle(studio).await;
                        let views = created
                            .iter()
                            .map(|(_, view)| view.as_str())
                            .collect::<Vec<_>>()
                            .join(", ");
                        studio.report_success_from(
                            MessageSource::Curation,
                            format!("Published {views}"),
                        );
                    }
                    Err(error) => studio.report_from(MessageSource::Curation, error),
                }
            }
            busy.set(false);
        });
    }

    /// Drop `node`'s view and forget that it is published.
    fn unpublish(self, node: String, view: String) {
        let Self {
            controller,
            studio,
            mut busy,
        } = self;
        let saved = controller.saved_revision();
        busy.set(true);
        spawn(async move {
            controller.report_activity(&node, Some(format!("Unpublishing {view}")));
            let request = HttpRequest::new("POST", "/api/curations/unpublish")
                .json(&UnpublishRequest::new(view));
            let forgotten = ProjectClient::current()
                .mutate::<()>(&[QueryDomain::Catalog], request)
                .await
                .map_err(|error| error.to_string())
                .and_then(|()| controller.edit(|payload| payload.remove_publication(&node, saved)));
            controller.report_activity(&node, None);
            match forgotten {
                Ok(()) => controller.save_when_idle(studio).await,
                Err(error) => studio.report_from(MessageSource::Curation, error),
            }
            busy.set(false);
        });
    }
}

/// The views one stored curation publishes, as its payload names them.
pub(in crate::activities::curation) struct PublishedViews(Vec<String>);

impl PublishedViews {
    pub(in crate::activities::curation) fn of(payload: &str) -> Self {
        Self(PublishedOutput::views_in(payload))
    }
    /// Drop every one of them, stopping at the first the catalog refuses.
    /// Dropping a view already gone succeeds, so a delete that stopped
    /// part-way can simply be asked again.
    pub(in crate::activities::curation) async fn drop_all(self) -> Result<(), String> {
        for view in self.0 {
            let request = HttpRequest::new("POST", "/api/curations/unpublish")
                .json(&UnpublishRequest::new(view));
            ProjectClient::current()
                .mutate::<()>(&[QueryDomain::Catalog], request)
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

/// Publish, at the end of the document bar, and the panel it opens: every
/// output with the name of its view, the ones to publish chosen.
#[component]
pub(in crate::activities::curation) fn CurationPublish(controller: CurationController) -> Element {
    let studio = StudioProjectState::current();
    let mut open = use_signal(|| false);
    let mut choice = use_signal(PublishChoice::default);
    let busy = use_signal(|| false);
    let publisher = ViewPublisher {
        controller,
        studio,
        busy,
    };
    use_effect(move || {
        if open() {
            BrowserFocus::after_render(PUBLISH_DIALOG_ID.to_owned());
        }
    });
    let outputs = PublishableOutput::all(controller);
    let none = outputs.is_empty();
    let unsaved = controller.unsaved_changes();
    let waiting = busy() || controller.command().is_some();
    let views = choice.read().views(&outputs);
    let count = choice.read().chosen.len();
    let close = move || {
        let mut open = open;
        open.set(false);
        BrowserFocus::after_render("curation-publish-toggle".to_owned());
    };
    rsx! {
        div { class: "curation-publish",
            Button {
                id: "curation-publish-toggle",
                purpose: ActionPurpose::Export,
                variant: ButtonVariant::Outline,
                aria_haspopup: "dialog",
                aria_expanded: open(),
                disabled: none,
                title: if none {
                    "Add Score progress, Find similar, Group similar, or a Select: each is an output that publishes as a view"
                } else {
                    "Choose outputs to publish as catalog views"
                },
                onclick: {
                    let outputs = outputs.clone();
                    move |_| {
                        if !open() {
                            choice.set(PublishChoice::of(&outputs));
                        }
                        open.toggle();
                    }
                },
                if busy() { "Publishing…" } else { "Publish" }
                ChevronIcon { open: open() }
            }
            if open() {
                div { class: "popover-backdrop", onclick: move |_| close() }
                div {
                    id: PUBLISH_DIALOG_ID,
                    class: "curation-publish-dialog",
                    role: "dialog",
                    tabindex: "-1",
                    "aria-label": "Publish outputs",
                    onkeydown: move |event: KeyboardEvent| {
                        if event.key() == Key::Escape {
                            event.prevent_default();
                            close();
                        }
                    },
                    p { class: "curation-publish-intro",
                        "Each output you choose becomes a catalog view of what it keeps, over the whole dataset."
                    }
                    if unsaved {
                        div { class: "curation-publish-unsaved", role: "status",
                            p { "Save the curation first: each view is recorded in it." }
                            Button {
                                purpose: ActionPurpose::Apply, variant: ButtonVariant::Outline,
                                disabled: controller.command().is_some(),
                                onclick: move |_| controller.save(studio),
                                "Save"
                            }
                        }
                    }
                    ul { class: "curation-publish-outputs",
                        for output in outputs.clone() {
                            PublishRow {
                                key: "{output.node}",
                                problem: choice.read().chosen.contains(&output.node)
                                    .then(|| choice.read().problem(&output, &outputs))
                                    .flatten(),
                                name: choice.read().name(&output.node).to_owned(),
                                chosen: choice.read().chosen.contains(&output.node),
                                current: controller.saved_revision(),
                                disabled: waiting,
                                on_choose: {
                                    let node = output.node.clone();
                                    move |chosen: bool| {
                                        let mut choice = choice.write();
                                        if chosen {
                                            choice.chosen.insert(node.clone());
                                        } else {
                                            choice.chosen.remove(&node);
                                        }
                                    }
                                },
                                on_name: {
                                    let node = output.node.clone();
                                    move |name: String| {
                                        choice.write().names.insert(node.clone(), name);
                                    }
                                },
                                on_unpublish: {
                                    let node = output.node.clone();
                                    move |view: String| publisher.unpublish(node.clone(), view)
                                },
                                output,
                            }
                        }
                    }
                    footer { class: "curation-publish-actions",
                        Button {
                            purpose: ActionPurpose::Cancel, variant: ButtonVariant::Link,
                            onclick: move |_| close(),
                            "Cancel"
                        }
                        Button {
                            purpose: ActionPurpose::Export, variant: ButtonVariant::Solid,
                            disabled: views.is_none() || unsaved || waiting,
                            title: if unsaved {
                                "Save the curation first"
                            } else if count == 0 {
                                "Choose the outputs to publish"
                            } else {
                                "Create or replace each chosen view under its name"
                            },
                            onclick: move |_| {
                                if let Some(views) = views.clone() {
                                    publisher.publish(views);
                                    choice.write().chosen.clear();
                                    close();
                                }
                            },
                            if count > 1 { "Publish {count} views" } else { "Publish" }
                        }
                    }
                }
            }
        }
    }
}

/// One output in the panel: whether to publish it, its card's words, the name
/// of its view, and where that view stands.
#[component]
fn PublishRow(
    output: PublishableOutput,
    name: String,
    chosen: bool,
    problem: Option<String>,
    /// The revision Studio holds the curation at, which a view is current
    /// against.
    current: u64,
    disabled: bool,
    on_choose: EventHandler<bool>,
    on_name: EventHandler<String>,
    on_unpublish: EventHandler<String>,
) -> Element {
    let unwritten = output.statement.as_ref().err().cloned();
    rsx! {
        li { class: "curation-publish-output", "data-chosen": "{chosen}",
            div { class: "curation-publish-output-head",
                Toggle {
                    label: output.label.to_owned(),
                    checked: chosen,
                    disabled: disabled || unwritten.is_some(),
                    title: unwritten.clone().unwrap_or_else(|| format!("Publish this {}", output.label)),
                    onchange: move |chosen| on_choose.call(chosen),
                }
                if let Some(published) = output.published.clone() {
                    if published.is_current(current) {
                        p { class: "curation-publish-status", role: "status",
                            title: "The catalog view {published.view()} is what the saved curation keeps here",
                            "Published as "
                            code { "{published.view()}" }
                        }
                    } else {
                        p { class: "curation-publish-status is-stale", role: "status",
                            title: "The saved curation has changed since {published.view()} was published; publish it again to update the view",
                            code { "{published.view()}" }
                            " is out of date"
                        }
                    }
                    Button {
                        purpose: ActionPurpose::Remove, variant: ButtonVariant::Link,
                        disabled,
                        title: "Drop the catalog view",
                        onclick: {
                            let view = published.view().to_owned();
                            move |_| on_unpublish.call(view.clone())
                        },
                        "Unpublish"
                    }
                }
            }
            p { class: "curation-publish-summary", "{output.summary}" }
            match unwritten {
                Some(reason) => rsx! {
                    p { class: "curation-publish-note", "{reason}" }
                },
                None => rsx! {
                    TextInput {
                        class: "curation-publish-name",
                        value: name,
                        placeholder: "view_name",
                        disabled,
                        "aria-label": "View name for this {output.label}",
                        "aria-invalid": problem.is_some().then_some("true"),
                        oninput: move |text| on_name.call(text),
                    }
                    if let Some(problem) = problem {
                        p { class: "curation-publish-problem", role: "alert", "{problem}" }
                    }
                },
            }
        }
    }
}
