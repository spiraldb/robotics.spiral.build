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
use super::source::ExplorationSource;
use crate::activities::workflow::build::document::{
    BuildNodeKind, DEFAULT_ANNOTATION_MODEL, SubtaskCategory, SubtaskGrouping, SubtaskRow,
};
use crate::activities::workflow::build::poster::{PosterCut, PosterFrames};
use crate::activities::workflow::build::rerun::ConfirmedRerun;
use crate::activities::workflow::build::review::{
    NoSubtaskChosen, SubtaskEntry, SubtaskEvidence, SubtaskReview, SubtaskStanding,
};
use crate::activities::workflow::build::state::{BuildController, EpisodeFocus};
use crate::client::model::{ModelClient, ModelRequest};
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use spiral_robotics_common::workflow::conversation::ProviderMessage;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, Disclosure, EmptyState, Field, FieldLayout, Notice,
    NoticeTone, Select, TextInput,
};

/// How many members a category card names before it is opened.
const REPRESENTATIVES: usize = 3;
/// The result-category washes Studio deals to repeated values. Six categories
/// is the most the grouping proposes, so every one gets its own.
const CATEGORY_HUES: usize = 6;
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
                "Putting rows in categories · {answered} of {requests} batches answered"
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

/// A category card's content: the category, how many rows joined it, the
/// share of the table that is, and the rows that joined it most typically.
#[derive(Clone, PartialEq)]
struct CategorySummary {
    category: SubtaskCategory,
    members: usize,
    /// Members as a fraction of every grouped row, so 91 and 12 do not read
    /// with the same weight.
    share: f64,
    /// Which of the six result-category washes this card wears, dealt by size
    /// so the same category keeps its colour while nothing is regrouped.
    hue: usize,
    representatives: Vec<String>,
}

impl CategorySummary {
    fn members_label(&self) -> String {
        if self.members == 1 {
            "1 subtask".to_owned()
        } else {
            format!("{} subtasks", self.members)
        }
    }
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
    let context = reading.context();
    let annotation = reading.annotation().unwrap_or_default().to_owned();
    let grouping = saved.read().clone();
    let open = opened.read().clone();
    let cards = grouping
        .as_ref()
        .map(|grouping| {
            let mut cards = grouping
                .categories()
                .iter()
                .map(|category| {
                    let members = grouping.members(category.name());
                    CategorySummary {
                        category: category.clone(),
                        members: members.len(),
                        share: if rows.is_empty() {
                            0.0
                        } else {
                            members.len() as f64 / rows.len() as f64
                        },
                        hue: 0,
                        representatives: members
                            .iter()
                            .take(REPRESENTATIVES)
                            .filter_map(|member| {
                                Some(rows[reading.row_of(member)?].description().to_owned())
                            })
                            .collect(),
                    }
                })
                .collect::<Vec<_>>();
            // The model emits its categories in no particular order. Biggest
            // first is the order an operator reads a taxonomy in, and it makes
            // the hue a card wears mean its size rather than its birth.
            cards.sort_by(|left, right| {
                right
                    .members
                    .cmp(&left.members)
                    .then_with(|| left.category.name().cmp(right.category.name()))
            });
            for (rank, card) in cards.iter_mut().enumerate() {
                card.hue = rank % CATEGORY_HUES;
            }
            cards
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
    let frames = PosterFrames::use_frames(PosterCut::of(&context, &rows));
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
    let chosen = selected_row();
    rsx! {
        section { class: "build-explore-workspace build-explore-grouping", "aria-label": "Group similar subtasks",
            if cards.is_empty() {
                EmptyState { title: "No categories yet",
                    p {
                        "One model pass reads the {rows.len()} saved descriptions and proposes four to eight categories; jev then puts every row in one of them."
                    }
                    Button {
                        purpose: ActionPurpose::Start,
                        variant: ButtonVariant::Solid,
                        disabled: busy || rows.is_empty(),
                        onclick: move |_| group.call(None),
                        if busy { "Generating…" } else { "Generate categories" }
                    }
                    if let Some(running) = running.clone() {
                        p { class: "build-explore-note", role: "status", "{running}" }
                    }
                }
            } else {
                div { class: "build-explore-actions",
                    p { class: "build-explore-note",
                        "{cards.len()} categories over {rows.len()} subtasks"
                    }
                    if let Some(running) = running.clone() {
                        p { class: "build-explore-note", role: "status", "{running}" }
                    }
                    ConfirmedRerun {
                        id: "build-explore-regenerate".to_owned(),
                        label: "Regenerate".to_owned(),
                        disabled: busy,
                        question: format!(
                            "Propose categories again? This replaces the {} categories and all {} assignments below.",
                            cards.len(),
                            rows.len(),
                        ),
                        on_confirm: move |()| group.call(None),
                    }
                }
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
            if let Some(problem) = problem {
                Notice { tone: NoticeTone::Danger, "{problem}" }
            }
            if let Some(edit) = open {
                SubtaskReview {
                    index: rsx! {
                        div { class: "build-explore-open",
                            h3 { class: "build-explore-open-name", "{edit.original()}" }
                            Disclosure { summary: rsx! { "Rename or merge" },
                                CategoryDetail {
                                    edit: edit.clone(),
                                    categories,
                                    busy,
                                    on_apply: EventHandler::new(move |edited: Vec<SubtaskCategory>| {
                                        group.call(Some(edited));
                                    }),
                                }
                            }
                        }
                        ol { class: "build-entries",
                            for (row , probability) in members {
                                SubtaskEntry {
                                    key: "{rows[row].episode()}-{rows[row].start_time()}",
                                    row: rows[row].clone(),
                                    standing: SubtaskStanding::scored(probability, true, chosen == Some(row)),
                                    episode: context.name(rows[row].episode()),
                                    frames,
                                    on_select: move |()| selected_row.set(Some(row)),
                                }
                            }
                        }
                    },
                    evidence: match chosen.and_then(|row| rows.get(row)) {
                        Some(row) => rsx! {
                            SubtaskEvidence {
                                key: "{row.episode()}-{row.start_time()}",
                                row: row.clone(),
                                context: context.clone(),
                                on_open: {
                                    let focus = EpisodeFocus::new(
                                        annotation.clone(),
                                        row.episode(),
                                        Some((row.start_time(), row.end_time())),
                                    );
                                    move |()| controller.focus_episode(focus.clone())
                                },
                            }
                        },
                        None => rsx! {
                            NoSubtaskChosen {
                                prompt: "Choose a member to watch it play, most typical first.",
                            }
                        },
                    },
                }
            }
        }
    }
}

/// One category: what it is, how many rows joined it, and the rows that
/// joined it most typically.
#[component]
fn CategoryCard(summary: CategorySummary, opened: bool, on_open: EventHandler<()>) -> Element {
    let share = (summary.share * 100.0).round();
    rsx! {
        // The examples are a list, and a list may not live inside a button, so
        // the button is the card's heading and the card is the article.
        article {
            class: "build-explore-category",
            style: "--hue: var(--tone-category-{summary.hue + 1})",
            "data-selected": "{opened}",
            button {
                r#type: "button",
                class: "build-explore-category-open",
                "aria-expanded": "{opened}",
                onclick: move |_| on_open.call(()),
                span { class: "build-explore-category-name", "{summary.category.name()}" }
                span { class: "build-explore-category-count", "{summary.members_label()}" }
            }
            div {
                class: "build-explore-category-share",
                "aria-hidden": "true",
                title: "{share}% of every grouped subtask",
                span { style: "width:{summary.share * 100.0}%" }
            }
            p { class: "build-explore-category-definition", "{summary.category.definition()}" }
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
                ConfirmedRerun {
                    id: "build-explore-merge".to_owned(),
                    label: "Merge".to_owned(),
                    disabled: busy || chosen.is_empty(),
                    question: format!(
                        "Merge “{}” into “{chosen}”? Its members are reassigned and the category is gone.",
                        edit.original(),
                    ),
                    on_confirm: move |()| {
                        apply.call(
                            CategoryEdits::new(&merging).merged(&chosen, draft.peek().original()),
                        );
                    },
                }
            }
            if let Some(problem) = problem() {
                Notice { tone: NoticeTone::Danger, "{problem}" }
            }
        }
    }
}
