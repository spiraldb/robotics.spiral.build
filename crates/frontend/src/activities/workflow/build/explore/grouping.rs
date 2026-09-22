//! Group similar subtasks: one LLM pass proposes the categories, one jev
//! Choice per row fills them.
//!
//! Rows are grouped, not episodes, so one episode can appear in several
//! categories. The categories are derived from these same descriptions, so
//! the list is complete by construction: every row joins its most probable
//! category, with no "other" option and no confidence gate. Renaming,
//! redefining, or merging a category reruns only the jev pass — the LLM
//! pass runs again only when the operator asks for new categories.
use super::categories::{
    AssignmentReader, CategoryEdit, CategoryEdits, DescriptionCounts, ProposedCategories,
};
use super::decisions::DecisionRun;
use super::questions::AssignmentQuestions;
use super::slice::{SliceActions, SliceScore, SubtaskCamera, SubtaskSlice};
use super::source::ExplorationSource;
use crate::activities::workflow::build::document::{
    BuildNodeKind, DEFAULT_ANNOTATION_MODEL, SubtaskCategory, SubtaskGrouping, SubtaskRow,
};
use crate::activities::workflow::build::state::{BuildController, EpisodeFocus};
use crate::client::model::{ModelClient, ModelRequest};
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::DatasetSelection;
use spiral_robotics_common::workflow::conversation::ProviderMessage;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, EmptyState, Field, FieldLayout, Notice, NoticeTone,
    Select, TextInput,
};

/// How many members a category card names before it is opened.
const REPRESENTATIVES: usize = 3;
const CATEGORY_ROLE: &str = "You name the categories of manipulation subtasks in a robot \
                             dataset. You answer with JSON and nothing else.";

/// One grouping run over one annotation's rows.
struct GroupingPass {
    node: String,
    annotation: String,
    rows: Vec<SubtaskRow>,
    model: ModelClient,
    /// The model the parent annotation used; grouping asks the same one.
    annotation_model: String,
}
impl GroupingPass {
    /// The LLM pass: 4 to 8 categories over the deduped descriptions.
    async fn propose(&self) -> Result<Vec<SubtaskCategory>, String> {
        let counts = DescriptionCounts::of(&self.descriptions());
        if counts.is_empty() {
            return Err("These rows have no descriptions to group.".into());
        }
        let request = ModelRequest::new(
            vec![
                ProviderMessage::text("system", CATEGORY_ROLE),
                ProviderMessage::text("user", counts.prompt()),
            ],
            Vec::new(),
        )
        .with_model(self.annotation_model.clone());
        let reply = self.model.complete(request, |_| {}).await?;
        let text = reply
            .content()
            .ok_or("The model answered the category prompt without any text.")?;
        Ok(ProposedCategories::parse(text)?.into_categories())
    }
    /// The jev pass: one Choice per row, batched, and the output to save.
    async fn assign(
        &self,
        categories: Vec<SubtaskCategory>,
        mut progress: impl FnMut(usize, usize),
    ) -> Result<SubtaskGrouping, String> {
        let run = DecisionRun::new(
            self.model.clone(),
            AssignmentQuestions::new(&categories)?.batches(&self.descriptions()),
        );
        let requests = run.batches();
        let reader = AssignmentReader::new(&self.rows, &categories);
        let mut assignments = Vec::with_capacity(self.rows.len());
        let mut answered = 0;
        progress(0, requests);
        run.run(|batch, answers| {
            assignments.extend(reader.read(batch, &answers));
            answered += 1;
            progress(answered, requests);
        })
        .await?;
        SubtaskGrouping::new(
            self.node.clone(),
            self.annotation.clone(),
            categories,
            assignments,
        )
    }
    fn descriptions(&self) -> Vec<&str> {
        self.rows.iter().map(SubtaskRow::description).collect()
    }
}

/// What a grouping run is doing, or why it stopped.
#[derive(Clone, PartialEq)]
enum GroupingStage {
    Proposing,
    Assigning { answered: usize, requests: usize },
    Failed(String),
}
impl GroupingStage {
    fn busy(&self) -> bool {
        !matches!(self, Self::Failed(_))
    }
    fn label(&self) -> Option<String> {
        match self {
            Self::Proposing => Some("Reading the subtask descriptions…".to_owned()),
            Self::Assigning { answered, requests } => Some(format!(
                "Assigning rows · {answered} of {requests} requests answered"
            )),
            Self::Failed(_) => None,
        }
    }
    fn problem(&self) -> Option<&str> {
        match self {
            Self::Failed(problem) => Some(problem),
            Self::Proposing | Self::Assigning { .. } => None,
        }
    }
}

/// A category card's content: the category, how many rows joined it, and
/// the descriptions of the rows that joined it most typically.
#[derive(Clone, PartialEq)]
struct CategorySummary {
    category: SubtaskCategory,
    members: usize,
    representatives: Vec<String>,
}

/// The workspace shown for a selected Group similar subtasks node.
#[component]
pub(in crate::activities::workflow::build) fn GroupingWorkspace(
    controller: BuildController,
    node: String,
) -> Element {
    let studio = StudioProjectState::current();
    let source = use_memo(use_reactive((&node,), move |(node,)| {
        ExplorationSource::of(controller, &node)
    }));
    let saved = use_memo(use_reactive((&node,), move |(node,)| {
        controller.with_payload(|payload| payload.grouping_for(&node).cloned())
    }));
    let mut stage = use_signal(|| Option::<GroupingStage>::None);
    let mut opened = use_signal(|| Option::<CategoryEdit>::None);
    let mut selected_row = use_signal(|| Option::<usize>::None);
    // A grouping and its open category belong to the node that produced
    // them, so both start again when the canvas selects another node.
    let mut showing = use_signal(|| Option::<String>::None);
    use_effect(use_reactive((&node,), move |(node,)| {
        if showing.peek().as_deref() == Some(node.as_str()) {
            return;
        }
        showing.set(Some(node.clone()));
        stage.set(None);
        opened.set(None);
        selected_row.set(None);
    }));

    let grouped_node = node.clone();
    let group = use_callback(move |categories: Option<Vec<SubtaskCategory>>| {
        if stage.peek().as_ref().is_some_and(GroupingStage::busy) {
            return;
        }
        let annotation = source.peek().annotation().map(str::to_owned);
        let Some(annotation) = annotation else {
            stage.set(Some(GroupingStage::Failed(
                "This grouping is not connected to a Break down subtasks node.".to_owned(),
            )));
            return;
        };
        let rows = source.peek().rows().to_vec();
        if rows.is_empty() {
            stage.set(Some(GroupingStage::Failed(
                "Run Break down subtasks first; grouping reads the rows it saved.".to_owned(),
            )));
            return;
        }
        let pass = GroupingPass {
            node: grouped_node.clone(),
            annotation_model: controller.with_payload(|payload| {
                match payload.node(&annotation).map(|node| node.kind()) {
                    Some(BuildNodeKind::BreakDownSubtasks(annotation)) => {
                        annotation.model().to_owned()
                    }
                    _ => DEFAULT_ANNOTATION_MODEL.to_owned(),
                }
            }),
            annotation,
            rows,
            model: ModelClient::current(),
        };
        stage.set(Some(match categories {
            Some(_) => GroupingStage::Assigning {
                answered: 0,
                requests: 0,
            },
            None => GroupingStage::Proposing,
        }));
        spawn(async move {
            let categories = match categories {
                Some(categories) => categories,
                None => match pass.propose().await {
                    Ok(categories) => categories,
                    Err(problem) => {
                        stage.set(Some(GroupingStage::Failed(problem)));
                        return;
                    }
                },
            };
            let assigned = pass
                .assign(categories, |answered, requests| {
                    stage.set(Some(GroupingStage::Assigning { answered, requests }));
                })
                .await;
            match assigned
                .and_then(|grouping| controller.edit(|payload| payload.set_grouping(grouping)))
            {
                Ok(()) => {
                    controller.save(studio);
                    stage.set(None);
                    opened.set(None);
                    selected_row.set(None);
                }
                Err(problem) => stage.set(Some(GroupingStage::Failed(problem))),
            }
        });
    });

    let reading = source.read();
    if reading.annotation().is_none() {
        return rsx! {
            section { class: "build-explore-workspace",
                Notice { tone: NoticeTone::Danger,
                    "This grouping is not connected to a Break down subtasks node."
                }
            }
        };
    }
    let rows = reading.rows().to_vec();
    let dataset = reading.dataset().cloned();
    let annotation = reading.annotation().unwrap_or_default().to_owned();
    let grouping = saved.read().clone();
    let open = opened.read().clone();
    let cards = grouping
        .as_ref()
        .map(|grouping| {
            grouping
                .categories()
                .iter()
                .map(|category| {
                    let members = grouping.members(category.name());
                    CategorySummary {
                        category: category.clone(),
                        members: members.len(),
                        representatives: members
                            .iter()
                            .take(REPRESENTATIVES)
                            .filter_map(|member| {
                                Some(rows[reading.row_of(member)?].description().to_owned())
                            })
                            .collect(),
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let members = match (&open, &grouping) {
        (Some(edit), Some(grouping)) => grouping
            .members(edit.original())
            .into_iter()
            .filter_map(|member| Some((reading.row_of(member)?, member.probability())))
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    drop(reading);
    let busy = stage.read().as_ref().is_some_and(GroupingStage::busy);
    let running = stage.read().as_ref().and_then(GroupingStage::label);
    let problem = stage
        .read()
        .as_ref()
        .and_then(GroupingStage::problem)
        .map(str::to_owned);
    let categories = cards
        .iter()
        .map(|card| card.category.clone())
        .collect::<Vec<_>>();
    rsx! {
        section { class: "build-explore-workspace build-explore-grouping", "aria-label": "Group similar subtasks",
            div { class: "build-explore-query",
                Button {
                    purpose: ActionPurpose::Start,
                    variant: ButtonVariant::Solid,
                    disabled: busy,
                    onclick: move |_| group.call(None),
                    if cards.is_empty() { "Generate categories" } else { "Regenerate categories" }
                }
                if let Some(running) = running {
                    p { class: "build-explore-note", role: "status", "{running}" }
                }
            }
            if let Some(problem) = problem {
                Notice { tone: NoticeTone::Danger, "{problem}" }
            }
            if cards.is_empty() {
                EmptyState { title: "No categories yet",
                    p {
                        "One model pass reads the saved subtask descriptions and proposes four to eight categories; jev then puts every row in one of them."
                    }
                }
            } else {
                div { class: "build-explore-categories",
                    for summary in cards {
                        CategoryCard {
                            key: "{summary.category.name()}",
                            summary: summary.clone(),
                            opened: open.as_ref().map(CategoryEdit::original) == Some(summary.category.name()),
                            on_open: EventHandler::new(move |()| {
                                let same = opened.peek().as_ref().map(CategoryEdit::original)
                                    == Some(summary.category.name());
                                opened.set((!same).then(|| CategoryEdit::of(&summary.category)));
                                selected_row.set(None);
                            }),
                        }
                    }
                }
            }
            if let Some(edit) = open {
                CategoryDetail {
                    edit,
                    categories,
                    busy,
                    on_apply: EventHandler::new(move |edited: Vec<SubtaskCategory>| {
                        group.call(Some(edited));
                    }),
                }
                ol { class: "build-explore-results",
                    for (row , probability) in members {
                        GroupedMember {
                            key: "{row}",
                            row: rows[row].clone(),
                            probability,
                            selected: selected_row() == Some(row),
                            dataset: dataset.clone(),
                            actions: SliceActions::new(
                                EventHandler::new(move |()| {
                                    let same = selected_row.peek().as_ref() == Some(&row);
                                    selected_row.set((!same).then_some(row));
                                }),
                                {
                                    let focus = EpisodeFocus::new(
                                        annotation.clone(),
                                        rows[row].episode(),
                                        Some((rows[row].start_time(), rows[row].end_time())),
                                    );
                                    EventHandler::new(move |()| controller.focus_episode(focus.clone()))
                                },
                            ),
                        }
                    }
                }
            }
        }
    }
}

/// One category: what it is, how many rows joined it, and the rows that
/// joined it most typically.
#[component]
fn CategoryCard(summary: CategorySummary, opened: bool, on_open: EventHandler<()>) -> Element {
    rsx! {
        button {
            class: "build-explore-category",
            "data-selected": "{opened}",
            "aria-pressed": "{opened}",
            onclick: move |_| on_open.call(()),
            span { class: "build-explore-category-name", "{summary.category.name()}" }
            span { class: "build-explore-category-count", "{summary.members} subtasks" }
            span { class: "build-explore-category-definition", "{summary.category.definition()}" }
            ul { class: "build-explore-category-examples",
                for representative in summary.representatives.iter() {
                    li { key: "{representative}", "{representative}" }
                }
            }
        }
    }
}

/// The open category's name, definition, and the merge that removes it.
/// Both actions rerun only the jev assignment.
#[component]
fn CategoryDetail(
    edit: CategoryEdit,
    categories: Vec<SubtaskCategory>,
    busy: bool,
    on_apply: EventHandler<Vec<SubtaskCategory>>,
) -> Element {
    let mut draft = use_signal(|| edit.clone());
    let mut problem = use_signal(|| Option::<String>::None);
    let mut target = use_signal(String::new);
    use_effect(use_reactive(&edit, move |edit| {
        draft.set(edit);
        problem.set(None);
        target.set(String::new());
    }));
    let apply = use_callback(
        move |edited: Result<Vec<SubtaskCategory>, String>| match edited {
            Ok(categories) => {
                problem.set(None);
                on_apply.call(categories);
            }
            Err(reason) => problem.set(Some(reason)),
        },
    );
    let others = categories
        .iter()
        .filter(|category| category.name() != edit.original())
        .map(|category| category.name().to_owned())
        .collect::<Vec<_>>();
    let renaming = categories.clone();
    let merging = categories;
    let chosen = target();
    rsx! {
        div { class: "build-explore-editor",
            Field { label: "Name", layout: FieldLayout::Stacked,
                TextInput {
                    value: draft.read().name().to_owned(),
                    oninput: move |value| draft.write().set_name(value),
                }
            }
            Field { label: "Definition", layout: FieldLayout::Stacked,
                TextInput {
                    value: draft.read().definition().to_owned(),
                    oninput: move |value| draft.write().set_definition(value),
                }
            }
            Button {
                purpose: ActionPurpose::Apply,
                variant: ButtonVariant::Outline,
                disabled: busy,
                onclick: move |_| apply.call(CategoryEdits::new(&renaming).renamed(&draft.read())),
                "Apply and reassign"
            }
            if !others.is_empty() {
                Field { label: "Merge into", layout: FieldLayout::Stacked,
                    Select {
                        value: chosen.clone(),
                        onchange: move |value| target.set(value),
                        option { value: "", "Choose a category" }
                        for other in others {
                            option { key: "{other}", value: "{other}", "{other}" }
                        }
                    }
                }
                Button {
                    purpose: ActionPurpose::Replace,
                    variant: ButtonVariant::Outline,
                    disabled: busy || chosen.is_empty(),
                    onclick: move |_| {
                        apply.call(
                            CategoryEdits::new(&merging).merged(&chosen, draft.peek().original()),
                        );
                    },
                    "Merge and reassign"
                }
            }
            if let Some(problem) = problem() {
                Notice { tone: NoticeTone::Danger, "{problem}" }
            }
        }
    }
}

/// One member of the open category, with its camera while it is selected.
#[component]
fn GroupedMember(
    row: SubtaskRow,
    probability: f64,
    selected: bool,
    dataset: Option<DatasetSelection>,
    actions: SliceActions,
) -> Element {
    let camera = match (selected, dataset) {
        (false, _) => rsx! {},
        (true, Some(dataset)) => rsx! {
            SubtaskCamera {
                dataset,
                episode: row.episode().to_owned(),
                interval: (row.start_time(), row.end_time()),
            }
        },
        (true, None) => rsx! {
            p { class: "build-explore-note", role: "status",
                "This Build document has no dataset, so the recording cannot be opened."
            }
        },
    };
    rsx! {
        li { class: "build-explore-result",
            SubtaskSlice {
                row,
                score: Some(SliceScore::new(probability, true)),
                selected,
                actions,
                {camera}
            }
        }
    }
}
