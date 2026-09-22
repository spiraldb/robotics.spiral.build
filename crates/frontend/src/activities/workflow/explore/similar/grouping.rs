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
use super::coverage::{CoverageMeasure, CoverageStrip, EpisodeCoverage, SubtaskList};
use super::decisions::DecisionRun;
use super::questions::AssignmentQuestions;
use super::source::ExplorationSource;
use crate::activities::workflow::explore::document::{
    DEFAULT_ANNOTATION_MODEL, ExplorationNodeKind, SubtaskCategory, SubtaskGrouping, SubtaskRow,
};
use crate::activities::workflow::explore::poster::{PosterCut, PosterFrames};
use crate::activities::workflow::explore::rerun::ConfirmedRerun;
use crate::activities::workflow::explore::review::{
    Counted, NoSubtaskChosen, SubtaskEntry, SubtaskEvidence, SubtaskReview, SubtaskStanding,
};
use crate::activities::workflow::explore::state::{EpisodeFocus, ExplorationController};
use crate::client::completions::{ModelClient, ModelRequest};
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
        let reply = ModelClient::complete(request, |_| {}).await?;
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
        let run =
            DecisionRun::new(AssignmentQuestions::new(&categories)?.batches(&self.descriptions()));
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

/// A category card's content: the category, the rows that joined it, the share
/// of the table that is, and which episodes they came from.
#[derive(Clone, PartialEq)]
struct CategorySummary {
    category: SubtaskCategory,
    /// The rows that joined it, most typical first, each with the probability
    /// that put it there, as the index lists them.
    members: Vec<(usize, f64)>,
    /// Members as a fraction of every grouped row, so 91 and 12 do not read
    /// with the same weight.
    share: f64,
    /// Which of the six result-category washes this card wears, dealt by size
    /// so the same category keeps its colour while nothing is regrouped.
    hue: usize,
    /// Which episodes those members came from, out of every episode the
    /// annotation measured.
    coverage: EpisodeCoverage,
    representatives: Vec<String>,
}

impl CategorySummary {
    /// Both counts a category is: how many rows joined it, and how much of the
    /// dataset they came from.
    fn counted(&self) -> String {
        format!(
            "{} · {}",
            Counted::subtasks(self.members.len()),
            self.coverage.episodes()
        )
    }
    /// The row the evidence column opens on: the most typical member.
    fn most_typical(&self) -> Option<usize> {
        self.members.first().map(|(row, _)| *row)
    }
    fn tint(&self) -> String {
        format!("--hue: var(--tone-category-{})", self.hue + 1)
    }
}

/// The workspace shown for a selected Group similar subtasks node.
#[component]
pub(in crate::activities::workflow::explore) fn GroupingWorkspace(
    controller: ExplorationController,
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
    // A grouping with nothing open is a band of cards over an empty half-pane,
    // which says nothing about the rows it just sorted. Arriving on one — a new
    // node, or a run that just finished — opens its biggest category on its
    // most typical member. Closing it afterwards stays closed.
    use_effect(use_reactive((&saved(),), move |(grouping,)| {
        let biggest = grouping.as_ref().and_then(|grouping| {
            let reading = source.peek();
            let mut sized = grouping
                .categories()
                .iter()
                .map(|category| (category, reading.listed(&grouping.members(category.name()))))
                .collect::<Vec<_>>();
            // The same order the card band reads in, so the category that opens
            // itself is the first one the operator sees.
            sized.sort_by(|left, right| {
                right
                    .1
                    .len()
                    .cmp(&left.1.len())
                    .then_with(|| left.0.name().cmp(right.0.name()))
            });
            sized.into_iter().next()
        });
        match biggest {
            Some((category, members)) => {
                opened.set(Some(CategoryEdit::of(category)));
                selected_row.set(members.first().map(|(row, _)| *row));
            }
            None => {
                opened.set(None);
                selected_row.set(None);
            }
        }
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
                    Some(ExplorationNodeKind::BreakDownSubtasks(annotation)) => {
                        annotation.model().to_owned()
                    }
                    _ => DEFAULT_ANNOTATION_MODEL.to_owned(),
                }
            }),
            annotation,
            rows,
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
                }
                Err(problem) => stage.set(Some(GroupingStage::Failed(problem))),
            }
        });
    });

    let reading = source.read();
    if reading.annotation().is_none() {
        return rsx! {
            section { class: "explore-similar-workspace",
                Notice { tone: NoticeTone::Danger,
                    "This grouping is not connected to a Break down subtasks node."
                }
            }
        };
    }
    let rows = reading.rows().to_vec();
    let context = reading.context();
    let measured = reading.measured();
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
                    let members = reading.listed(&grouping.members(category.name()));
                    CategorySummary {
                        category: category.clone(),
                        share: if rows.is_empty() {
                            0.0
                        } else {
                            members.len() as f64 / rows.len() as f64
                        },
                        hue: 0,
                        coverage: EpisodeCoverage::of(
                            &measured,
                            members
                                .iter()
                                .map(|(row, probability)| (rows[*row].episode(), *probability)),
                            CoverageMeasure::Members,
                        ),
                        representatives: members
                            .iter()
                            .take(REPRESENTATIVES)
                            .map(|(row, _)| rows[*row].description().to_owned())
                            .collect(),
                        members,
                    }
                })
                .collect::<Vec<_>>();
            // The model emits its categories in no particular order. Biggest
            // first is the order an operator reads a taxonomy in, and it makes
            // the hue a card wears mean its size rather than its birth.
            cards.sort_by(|left, right| {
                right
                    .members
                    .len()
                    .cmp(&left.members.len())
                    .then_with(|| left.category.name().cmp(right.category.name()))
            });
            for (rank, card) in cards.iter_mut().enumerate() {
                card.hue = rank % CATEGORY_HUES;
            }
            cards
        })
        .unwrap_or_default();
    drop(reading);
    let open_card = open
        .as_ref()
        .and_then(|edit| {
            cards
                .iter()
                .find(|card| card.category.name() == edit.original())
        })
        .cloned();
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
    let showing = chosen
        .and_then(|row| rows.get(row))
        .map(|row| row.episode().to_owned());
    // An open category needs the height for its members and their footage, so
    // the card band steps back to one row of chips while one is open.
    let band = if open_card.is_some() {
        "explore-similar-chips"
    } else {
        "explore-similar-categories"
    };
    // The open category, taken apart here so the markup below reads as a shape
    // rather than a run of field moves.
    let tint = open_card
        .as_ref()
        .map(CategorySummary::tint)
        .unwrap_or_default();
    let members = open_card
        .as_ref()
        .map(|card| card.members.clone())
        .unwrap_or_default();
    // The strip points at places in the member list; the table row that sits
    // in each of them is this view's own business.
    let member_rows = members.iter().map(|(row, _)| *row).collect::<Vec<_>>();
    let coverage = open_card.map(|card| card.coverage);
    rsx! {
        section { class: "explore-similar-workspace explore-similar-grouping", "aria-label": "Group similar subtasks",
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
                        p { class: "explore-similar-note", role: "status", "{running}" }
                    }
                }
            } else {
                div { class: "explore-similar-actions",
                    p { class: "explore-similar-note",
                        "{cards.len()} categories over {rows.len()} subtasks"
                    }
                    if let Some(running) = running.clone() {
                        p { class: "explore-similar-note", role: "status", "{running}" }
                    }
                    ConfirmedRerun {
                        id: "explore-similar-regenerate".to_owned(),
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
                div { class: "{band}",
                    for summary in cards {
                        CategoryCard {
                            key: "{summary.category.name()}",
                            summary: summary.clone(),
                            opened: open.as_ref().map(CategoryEdit::original) == Some(summary.category.name()),
                            collapsed: band == "explore-similar-chips",
                            on_open: EventHandler::new(move |()| {
                                let same = opened.peek().as_ref().map(CategoryEdit::original)
                                    == Some(summary.category.name());
                                opened.set((!same).then(|| CategoryEdit::of(&summary.category)));
                                // Opening a category lands on its most typical
                                // member, so the evidence column is never an
                                // empty half beside a full list.
                                selected_row.set((!same).then(|| summary.most_typical()).flatten());
                            }),
                        }
                    }
                }
            }
            if let Some(problem) = problem {
                Notice { tone: NoticeTone::Danger, "{problem}" }
            }
            if let (Some(edit), Some(coverage)) = (open, coverage) {
                SubtaskReview {
                    index: rsx! {
                        div { class: "explore-similar-open", style: "{tint}",
                            // The band above stepped back to chips to make room
                            // for these members, and the definition is what it
                            // gave up, so the open category carries it.
                            h3 { class: "explore-similar-open-name",
                                "{edit.original()}"
                                span { class: "explore-similar-open-definition", "{edit.definition()}" }
                            }
                            CoverageStrip {
                                coverage,
                                list: SubtaskList::MEMBERS,
                                showing,
                                on_reveal: move |place: Option<usize>| {
                                    selected_row
                                        .set(place.and_then(|place| member_rows.get(place).copied()));
                                },
                            }
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
                        ol {
                            id: SubtaskList::MEMBERS.id(),
                            class: "explore-entries",
                            "aria-label": "Members of this category, most typical first; each row ends with how sure jev is it belongs here",
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
                                episode_rows: rows
                                    .iter()
                                    .filter(|other| other.episode() == row.episode())
                                    .cloned()
                                    .collect::<Vec<_>>(),
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

/// One category: what it is, how many rows joined it, which episodes they came
/// from, and the rows that joined it most typically.
///
/// While a category is open the band has no height to spare for six of these,
/// so every card steps back to its name and its counts alone.
#[component]
fn CategoryCard(
    summary: CategorySummary,
    opened: bool,
    /// One row of chips rather than a band of cards, because an open category
    /// owns the height.
    collapsed: bool,
    on_open: EventHandler<()>,
) -> Element {
    let share = format!("{:.0}%", summary.share * 100.0);
    let counted = summary.counted();
    if collapsed {
        // The chip carries the name and the size; the open category below it
        // is already saying which episodes it reached, and six names plus six
        // pairs of counts would cost the rows this band just gave back.
        let members = Counted::subtasks(summary.members.len());
        return rsx! {
            button {
                r#type: "button",
                class: "explore-similar-chip",
                style: "{summary.tint()}",
                "aria-expanded": "{opened}",
                "data-selected": "{opened}",
                onclick: move |_| on_open.call(()),
                span { class: "explore-similar-chip-name", "{summary.category.name()}" }
                span { class: "explore-similar-chip-count", "{members}" }
            }
        };
    }
    rsx! {
        // The examples are a list, and a list may not live inside a button, so
        // the button is the card's heading and the card is the article.
        article {
            class: "explore-similar-category",
            style: "{summary.tint()}",
            "data-selected": "{opened}",
            button {
                r#type: "button",
                class: "explore-similar-category-open",
                "aria-expanded": "{opened}",
                onclick: move |_| on_open.call(()),
                span { class: "explore-similar-category-name", "{summary.category.name()}" }
            }
            div {
                class: "explore-similar-category-share",
                "aria-hidden": "true",
                span { style: "width:{summary.share * 100.0}%" }
            }
            // The share the bar draws, the rows it counted, and the episodes
            // they came from — the bar is never the only place the number is.
            p { class: "explore-similar-category-count", "{share} · {counted}" }
            p { class: "explore-similar-category-definition", "{summary.category.definition()}" }
            ul { class: "explore-similar-category-examples",
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
        div { class: "explore-similar-editor",
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
                    id: "explore-similar-merge".to_owned(),
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
