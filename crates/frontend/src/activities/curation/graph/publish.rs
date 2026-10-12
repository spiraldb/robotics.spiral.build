//! Publishing from the document bar: the nodes the operator chooses, each
//! compiled over every episode of the dataset and published as a view under
//! the name they give it, then recorded in the curation, which saves.
//! A view cannot be turned back into a graph, so the recipe stays in the
//! curation and the view is what it produces.
//!
//! A view reads the answers its models stored, so publishing is one fill run
//! on the backend: it asks each model about every episode that reaches its
//! node and has no answer yet, then creates the views. The run goes on after
//! the press, and Data shows each view as in progress until it is done.
//!
//! A publication is recorded against the saved recipe, and the save that
//! records it must come next. So one press saves what is drawn, starts the
//! run, records every chosen view in one edit, and saves again; and nothing
//! else publishes or unpublishes until that is done.
use super::node_card::NodeCard;
use crate::activities::curation::client::CurationClient;
use crate::activities::curation::document::{NodePublication, PublishedOutput};
use crate::activities::curation::path::{CurationPath, PathError};
use crate::activities::curation::state::CurationController;
use crate::client::project::ProjectClient;
use crate::client::queries::{QueryDomain, QueryRefresh};
use crate::client::transport::HttpRequest;
use crate::shell::notifications::MessageSource;
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use spiral_robotics_common::curation::{
    ColumnFill, FillRequest, PublishedView, UnpublishRequest, ViewName,
};
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_ui::{
    ActionPurpose, BrowserFocus, Button, ButtonVariant, ChevronIcon, TextInput, Toggle,
};
use std::collections::{BTreeMap, BTreeSet};

/// Focus has to be inside the panel for its own Escape handler to hear the
/// key, the way the document switcher's is.
const PUBLISH_DIALOG_ID: &str = "curation-publish-dialog";

/// What an output publishes as: the statement its view is, and every column
/// of stored answers the view reads, in the order a fill writes them.
#[derive(Clone, Debug, PartialEq)]
struct CompiledOutput {
    statement: String,
    fills: Vec<ColumnFill>,
}

impl CompiledOutput {
    fn of(path: &CurationPath<'_>) -> Result<Self, PathError> {
        Ok(Self {
            statement: path.statement()?,
            fills: path.fills()?,
        })
    }
}

/// One output as the panel lists it: its node, how its card reads, what it
/// publishes as, or why it cannot be, and the view it is published as.
#[derive(Clone, Debug, PartialEq)]
struct PublishableOutput {
    node: String,
    label: &'static str,
    summary: String,
    compiled: Result<CompiledOutput, String>,
    published: Option<NodePublication>,
}

/// One chosen output: its node, and the view it publishes as with the
/// answers it reads.
#[derive(Clone, Debug, PartialEq)]
struct ChosenView {
    node: String,
    view: PublishedView,
    fills: Vec<ColumnFill>,
}

impl ChosenView {
    /// The one run publishing `views` over `dataset`: every column they
    /// read, each once, in the order a fill writes them.
    fn run(dataset: DatasetSelection, views: &[Self]) -> FillRequest {
        let mut columns = Vec::<ColumnFill>::new();
        for fill in views.iter().flat_map(|chosen| chosen.fills.iter()) {
            if !columns.contains(fill) {
                columns.push(fill.clone());
            }
        }
        let published = views.iter().map(|chosen| chosen.view.clone()).collect();
        FillRequest::new(dataset, columns).publishing(published)
    }
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
                        compiled: CurationPath::to(payload, output.id())
                            .and_then(|path| CompiledOutput::of(&path))
                            .map_err(|error| error.message().to_owned()),
                        published: payload.node_publication(output.id()),
                    })
                })
                .collect()
        })
    }
}

/// What the operator has chosen while the panel is open: the name typed for
/// each output, starting from the view it is published as, and which outputs
/// to publish; and whether an unnamed view has become worth pointing out.
#[derive(Clone, Debug, Default, PartialEq)]
struct PublishChoice {
    names: BTreeMap<String, String>,
    chosen: BTreeSet<String>,
    /// Outputs whose name field the operator has left.
    left: BTreeSet<String>,
    /// Whether Publish was pressed while a chosen view could not be created.
    attempted: bool,
}

impl PublishChoice {
    fn of(outputs: &[PublishableOutput]) -> Self {
        Self {
            names: outputs
                .iter()
                .map(|output| {
                    let name = output.published.as_ref().map(NodePublication::view);
                    (output.node.clone(), name.unwrap_or_default().to_owned())
                })
                .collect(),
            ..Self::default()
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
    /// The problem to show beside `output`'s name. A view not yet named is
    /// only unfinished until the operator leaves its field empty or presses
    /// Publish; a name they typed is judged as they type it.
    fn shown_problem(
        &self,
        output: &PublishableOutput,
        outputs: &[PublishableOutput],
    ) -> Option<String> {
        if !self.chosen.contains(&output.node) {
            return None;
        }
        let problem = self.problem(output, outputs)?;
        let due = !self.name(&output.node).is_empty()
            || self.attempted
            || self.left.contains(&output.node);
        due.then_some(problem)
    }
    /// The chosen outputs as views to create, once every one of them can be.
    fn views(&self, outputs: &[PublishableOutput]) -> Option<Vec<ChosenView>> {
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
                let compiled = output.compiled.clone().ok()?;
                Some(ChosenView {
                    node: output.node.clone(),
                    view: PublishedView::new(self.name(&output.node), compiled.statement),
                    fills: compiled.fills,
                })
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
    /// Save what is drawn, since Data judges a view against the stored
    /// recipe; then start the one run filling every column the chosen views
    /// read and creating them, record every view in one edit, and save again.
    fn publish(self, views: Vec<ChosenView>) {
        let Self {
            controller,
            studio,
            mut busy,
        } = self;
        busy.set(true);
        spawn(async move {
            if !controller.saved(studio).await {
                busy.set(false);
                return;
            }
            let names = views
                .iter()
                .map(|chosen| chosen.view.name().to_owned())
                .collect::<Vec<_>>()
                .join(", ");
            for chosen in &views {
                controller.report_activity(
                    &chosen.node,
                    Some(format!("Publishing as {}", chosen.view.name())),
                );
            }
            let dataset = controller.with_payload(|payload| payload.dataset().cloned());
            let started = match dataset {
                Some(dataset) => CurationClient::current()
                    .start_fill(&ChosenView::run(dataset, &views))
                    .await
                    .map_err(|error| error.to_string()),
                None => Err("Choose the dataset this curation reads.".to_owned()),
            };
            for chosen in &views {
                controller.report_activity(&chosen.node, None);
            }
            let recorded = started.and_then(|_| {
                controller.edit(|payload| {
                    views.iter().try_for_each(|chosen| {
                        payload.record_publication(&chosen.node, &chosen.view)
                    })
                })
            });
            match recorded {
                Ok(()) => {
                    controller.save_when_idle(studio).await;
                    // Data lists a view by the curation recording it, and
                    // shows how far its run has got.
                    QueryRefresh::current().invalidate(&[QueryDomain::Catalog]);
                    studio.report_success_from(
                        MessageSource::Curation,
                        format!("Publishing {names}. Data shows how far its models have got."),
                    );
                }
                Err(error) => studio.report_from(MessageSource::Curation, error),
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
        busy.set(true);
        spawn(async move {
            controller.report_activity(&node, Some(format!("Unpublishing {view}")));
            let forgotten = PublishedViews::drop_view(view)
                .await
                .and_then(|()| controller.edit(|payload| payload.remove_publication(&node)));
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
    /// Drop every one of them, stopping at the first that fails.
    /// Dropping a view already gone succeeds, so a delete that stopped
    /// part-way can simply be asked again.
    pub(in crate::activities::curation) async fn drop_all(self) -> Result<(), String> {
        for view in self.0 {
            Self::drop_view(view).await?;
        }
        Ok(())
    }
    /// Drop the published view `view`; one already gone is dropped too.
    pub(in crate::activities::curation) async fn drop_view(view: String) -> Result<(), String> {
        let request =
            HttpRequest::new("POST", "/api/curations/unpublish").json(&UnpublishRequest::new(view));
        ProjectClient::current()
            .mutate::<()>(&[QueryDomain::Catalog], request)
            .await
            .map_err(|error| error.to_string())
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
                    "Add a node after the dataset: every node but the dataset publishes as a view"
                } else {
                    "Choose nodes to publish as views"
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
                    ul { class: "curation-publish-outputs",
                        for output in outputs.clone() {
                            PublishRow {
                                key: "{output.node}",
                                problem: choice.read().shown_problem(&output, &outputs),
                                name: choice.read().name(&output.node).to_owned(),
                                chosen: choice.read().chosen.contains(&output.node),
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
                                on_leave: {
                                    let node = output.node.clone();
                                    move |()| {
                                        choice.write().left.insert(node.clone());
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
                            disabled: count == 0 || waiting,
                            title: if count == 0 {
                                "Choose the outputs to publish"
                            } else {
                                "Save the curation, then store its models' answers and create or replace each chosen view under its name"
                            },
                            // A chosen view that cannot be created yet says why.
                            onclick: move |_| match views.clone() {
                                Some(views) => {
                                    publisher.publish(views);
                                    choice.write().chosen.clear();
                                    close();
                                }
                                None => choice.write().attempted = true,
                            },
                            if count > 1 { "Publish {count} views" } else { "Publish" }
                        }
                    }
                }
            }
        }
    }
}

/// One output in the panel: whether to publish it and where its view stands,
/// then — hanging from the checkbox's label — its card's words and the name of
/// its view.
#[component]
fn PublishRow(
    output: PublishableOutput,
    name: String,
    chosen: bool,
    problem: Option<String>,
    disabled: bool,
    on_choose: EventHandler<bool>,
    on_name: EventHandler<String>,
    /// The name field lost focus.
    on_leave: EventHandler<()>,
    on_unpublish: EventHandler<String>,
) -> Element {
    let unwritten = output.compiled.as_ref().err().cloned();
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
                // The view's name is the field under the row, so the state
                // is one word at the row's end, as the card says it.
                if let Some(published) = output.published.clone() {
                    if published.is_current() {
                        p { class: "curation-publish-status is-published", role: "status",
                            title: "Published as {published.view()}: the view is what this node keeps",
                            "Published"
                        }
                    } else {
                        p { class: "curation-publish-status is-stale", role: "status",
                            title: "What this node keeps has changed since {published.view()} was published; publish it again to update the view",
                            "Out of date"
                        }
                    }
                    Button {
                        purpose: ActionPurpose::Remove, variant: ButtonVariant::Link,
                        disabled,
                        title: "Drop the view",
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
                        onblur: move |_| on_leave.call(()),
                    }
                    if let Some(problem) = problem {
                        p { class: "curation-publish-problem", role: "alert", "{problem}" }
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ChosenView, CompiledOutput, PublishChoice, PublishableOutput};
    use spiral_robotics_common::curation::{
        AnswerColumn, AnswerSource, ColumnFill, FillEpisodes, PublishedView,
    };
    use spiral_robotics_common::datasets::DatasetSelection;

    #[test]
    fn one_run_publishes_every_chosen_view_and_fills_what_they_share_once() {
        let fill = |node: &str, reaching: &str| {
            ColumnFill::new(
                node,
                AnswerColumn::of("probe", &[node]).unwrap(),
                FillEpisodes::Reaching(reaching.to_owned()),
                AnswerSource::Statement {
                    statement: "SELECT $1 AS episode_id, 1 AS answer".to_owned(),
                },
            )
        };
        let (scores, subtasks) = (fill("Score progress", "all"), fill("Break down", "kept"));
        let views = [
            ChosenView {
                node: "score_progress-1".into(),
                view: PublishedView::new("moving", "SELECT 1"),
                fills: vec![scores.clone()],
            },
            ChosenView {
                node: "group-1".into(),
                view: PublishedView::new("folds", "SELECT 2"),
                fills: vec![scores.clone(), subtasks.clone()],
            },
        ];
        let dataset = DatasetSelection::new(vec!["dataset_folding".into()]).unwrap();
        let run = ChosenView::run(dataset, &views);
        assert_eq!(run.columns(), [scores, subtasks]);
        assert_eq!(
            run.views()
                .iter()
                .map(PublishedView::name)
                .collect::<Vec<_>>(),
            ["moving", "folds"]
        );
    }

    fn output(node: &str) -> PublishableOutput {
        PublishableOutput {
            node: node.to_owned(),
            label: "Score progress",
            summary: String::new(),
            compiled: Ok(CompiledOutput {
                statement: "SELECT 1".to_owned(),
                fills: Vec::new(),
            }),
            published: None,
        }
    }

    #[test]
    fn an_unnamed_view_is_pointed_out_only_once_left_empty_or_published() {
        let outputs = vec![output("score_progress-1")];
        let mut choice = PublishChoice::of(&outputs);
        choice.chosen.insert("score_progress-1".to_owned());
        assert_eq!(choice.shown_problem(&outputs[0], &outputs), None);
        assert!(
            choice.views(&outputs).is_none(),
            "it still cannot be published"
        );

        let mut left = choice.clone();
        left.left.insert("score_progress-1".to_owned());
        assert_eq!(
            left.shown_problem(&outputs[0], &outputs).as_deref(),
            Some("Name the view.")
        );

        let mut attempted = choice.clone();
        attempted.attempted = true;
        assert_eq!(
            attempted.shown_problem(&outputs[0], &outputs).as_deref(),
            Some("Name the view.")
        );
    }

    #[test]
    fn a_typed_name_is_judged_as_it_is_typed() {
        let outputs = vec![output("score_progress-1")];
        let mut choice = PublishChoice::of(&outputs);
        choice
            .names
            .insert("score_progress-1".to_owned(), "Not a view!".to_owned());
        assert_eq!(
            choice.shown_problem(&outputs[0], &outputs),
            None,
            "unchosen outputs say nothing"
        );
        choice.chosen.insert("score_progress-1".to_owned());
        assert!(choice.shown_problem(&outputs[0], &outputs).is_some());
        choice.names.insert(
            "score_progress-1".to_owned(),
            "long_demos_steady".to_owned(),
        );
        assert_eq!(choice.shown_problem(&outputs[0], &outputs), None);
        assert!(choice.views(&outputs).is_some());
    }
}
