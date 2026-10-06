//! Group similar subtasks: one LLM pass in the browser proposes the
//! categories as soon as the subtasks that reach the node have arrived, they
//! become the node's configuration, and the node's step puts
//! every subtask that reaches it in one of them with `spql.ai.classify`. Its
//! preview runs that step over the subtasks Break down's preview holds, many
//! episodes to a query, as the curation's own run.
//!
//! Rows are grouped, not episodes, so one episode can appear in several
//! categories. The categories are derived from these same descriptions, so the
//! list is complete by construction: every row joins its most probable
//! category, with no "other" option and no confidence gate. Renaming or
//! merging a category runs only the classification again — the LLM pass runs
//! again only when the operator asks for new categories.
//!
//! The node is an output, and what it hands on is the subtasks of the
//! categories it keeps: all of them once proposed, and each category's card
//! says whether it is kept and leaves it out when unchecked, which asks
//! nothing again.
use super::categories::{CategoryEdit, DescriptionCounts, ProposedCategories};
use super::coverage::{CoverageMeasure, EpisodeCoverage, SubtaskList};
use super::profile::{CategoryProfile, ProfilePanel};
use super::source::SubtaskSource;
use crate::activities::curation::annotate::SubtaskRow;
use crate::activities::curation::document::{
    CurationNode, CurationNodeKind, DEFAULT_ANNOTATION_MODEL, Selection, SubtaskCategory,
};
use crate::activities::curation::episode_map::{
    EpisodeLanes, EpisodeMap, EpisodeSpans, MapPaint, MapReveal, SegmentPaint,
};
use crate::activities::curation::episode_tabs::{
    EpisodeTabs, LeadingTab, OpenEpisode, OpenEpisodes,
};
use crate::activities::curation::population::Slice;
use crate::activities::curation::rerun::ConfirmedRerun;
use crate::activities::curation::review::{
    Counted, FailedEpisodes, NodeBar, NodeNotes, NodeReach, RowKeys, RowStanding, SubtaskEntry,
    SubtaskEvidence, SubtaskIntervals, SubtaskReview, TabStop,
};
use crate::activities::curation::state::{CurationController, EpisodeFocus, RunState};
use crate::activities::curation::thumbnail::Thumbnails;
use crate::activities::episode::Episode;
use crate::client::completions::{ModelClient, ModelRequest};
use dioxus::prelude::*;
use spiral_robotics_common::providers::ProviderMessage;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonVariant, Disclosure, EmptyState, Field, FieldLayout, Flight,
    Motion, Notice, NoticeTone, Place, Select, TextInput,
};

/// How many members a category card names before it is opened.
const REPRESENTATIVES: usize = 3;
/// The result-category washes Studio deals to repeated values. Six categories
/// is the most the grouping proposes, so every one gets its own.
const CATEGORY_HUES: usize = 6;
/// The order those washes are dealt, biggest category first, as 1-based tone
/// numbers: away from blue, which already means focus, and reaching it last
/// (DESIGN.md's Blue-Comes-Last Rule).
const DEALT_TONES: [usize; CATEGORY_HUES] = [2, 4, 3, 5, 6, 1];
const CATEGORY_ROLE: &str = "You name the categories of manipulation subtasks in a robot \
                             dataset. You answer with JSON and nothing else.";

/// One Group similar subtasks node, read and configured through the
/// controller.
#[derive(Clone, PartialEq)]
struct GroupingNode {
    id: String,
    controller: CurationController,
}

impl GroupingNode {
    /// The categories the node groups by; none before any are proposed.
    fn categories(&self) -> Vec<SubtaskCategory> {
        self.controller.with_payload(|payload| {
            match payload.node(&self.id).map(CurationNode::kind) {
                Some(CurationNodeKind::GroupSimilarSubtasks(grouping)) => {
                    grouping.categories().to_vec()
                }
                _ => Vec::new(),
            }
        })
    }
    /// Whether a node above is still running, so the subtasks that reach
    /// this one are still arriving.
    fn subtasks_arriving(&self) -> bool {
        let parent = |node: &str| {
            self.controller
                .with_payload(|payload| payload.parent_of(node).map(str::to_owned))
        };
        let mut above = parent(&self.id);
        while let Some(node) = above {
            if self.controller.run_state(&node) != RunState::Idle {
                return true;
            }
            above = parent(&node);
        }
        false
    }
    /// The LLM pass: 4 to 8 categories over the deduped descriptions of the
    /// subtasks that reach the node, asked of the model that described them.
    async fn propose(descriptions: Vec<String>) -> Result<Vec<SubtaskCategory>, String> {
        let borrowed = descriptions.iter().map(String::as_str).collect::<Vec<_>>();
        let counts = DescriptionCounts::of(&borrowed);
        if counts.is_empty() {
            return Err("These rows have no descriptions to group.".into());
        }
        let request = ModelRequest::new(
            vec![
                ProviderMessage::text("system", CATEGORY_ROLE),
                ProviderMessage::text("user", counts.prompt()),
            ],
            Vec::new(),
            DEFAULT_ANNOTATION_MODEL,
        );
        let reply = ModelClient::complete(request).await?;
        let text = reply
            .content()
            .ok_or("The model answered the category prompt without any text.")?;
        Ok(ProposedCategories::parse(text)?.into_categories())
    }
    /// The names of the categories the node keeps, in its list's order.
    fn kept(&self) -> Vec<String> {
        self.controller.with_payload(|payload| {
            match payload.node(&self.id).and_then(CurationNode::keeps) {
                Some(Selection::Categories { names }) => names.clone(),
                _ => Vec::new(),
            }
        })
    }
    /// Keep the category called `name`, or leave it out. The rows are
    /// grouped already, so nothing is asked again.
    fn keep(&self, name: &str, keep: bool) -> Result<(), String> {
        let kept = self.kept();
        let names = self
            .categories()
            .iter()
            .map(SubtaskCategory::name)
            .filter(|category| {
                if *category == name {
                    keep
                } else {
                    kept.iter().any(|kept| kept == category)
                }
            })
            .map(str::to_owned)
            .collect();
        self.controller
            .set_rule(&self.id, Selection::Categories { names })
    }
    /// Change the categories, then run the node's step on every episode that
    /// reaches it: a changed list has classified nothing by it yet.
    fn regroup(&self, change: Regroup) -> Result<(), String> {
        let id = self.id.clone();
        self.controller.edit(|payload| {
            let node = payload
                .node_mut(&id)
                .ok_or("This node is no longer in the graph.")?;
            match change {
                Regroup::Proposed(categories) => node.propose_categories(categories),
                Regroup::Edited { original, edited } => node.edit_category(&original, edited),
                Regroup::Merged { merged, into } => node.merge_category(&merged, &into),
            }
        })?;
        self.run()
    }
    /// Classify the subtasks of every episode that reaches the node and has
    /// not been grouped from them, or whose grouping failed.
    fn run(&self) -> Result<(), String> {
        let Some(progress) = self.controller.progress(&self.id) else {
            return Ok(());
        };
        let mut episodes = progress.pending();
        episodes.extend(progress.failed().iter().map(|(episode, _)| episode.clone()));
        if episodes.is_empty() {
            return Ok(());
        }
        self.controller.run_preview(&self.id, &episodes)
    }
}

/// A change to a grouping's categories, each of which groups every subtask
/// again. What the grouping keeps follows the change: a proposed list is
/// kept whole, an edited category keeps its standing under its new name, and
/// a merged one joins the other's.
#[derive(Clone, Debug, PartialEq)]
enum Regroup {
    Proposed(Vec<SubtaskCategory>),
    Edited {
        original: String,
        edited: SubtaskCategory,
    },
    Merged {
        merged: String,
        into: String,
    },
}

/// Which colour, and which place in the band, each category gets: biggest
/// first, the order an operator reads a taxonomy in, so the hue a category
/// wears means its size rather than the order the model happened to name it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct CategoryHues(Vec<String>);

impl CategoryHues {
    /// The hues of `categories`, ranked by how many of `rows` joined each.
    pub(super) fn of_rows(categories: &[SubtaskCategory], rows: &[SubtaskRow]) -> Self {
        let mut sized = categories
            .iter()
            .map(|category| {
                let members = rows
                    .iter()
                    .filter(|row| row.category() == Some(category.name()))
                    .count();
                (category.name().to_owned(), members)
            })
            .collect::<Vec<_>>();
        sized.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        Self(sized.into_iter().map(|(name, _)| name).collect())
    }
    /// Where a category stands in the band, biggest first.
    fn rank(&self, category: &str) -> Option<usize> {
        self.0.iter().position(|name| name == category)
    }
    /// The 1-based wash a category of this size rank wears.
    fn tone_of_rank(rank: usize) -> usize {
        DEALT_TONES[rank % CATEGORY_HUES]
    }
    fn hue(&self, category: &str) -> Option<usize> {
        self.rank(category).map(|rank| Self::tone_of_rank(rank) - 1)
    }
    /// The inline style that lends a category's hue to whatever wears it;
    /// nothing for no category, which falls back to the neutral line colour.
    pub(super) fn tint_of(&self, category: Option<&str>) -> String {
        category
            .and_then(|category| self.hue(category))
            .map(|hue| format!("--hue: var(--tone-category-{})", hue + 1))
            .unwrap_or_default()
    }
}

/// The rows a finished grouping sends flying, each from its segment of the
/// map to its category's place in the band: the motion is the run landing.
#[derive(Clone, Debug, Default, PartialEq)]
struct PourPlan(Vec<(usize, String)>);

impl PourPlan {
    fn of(rows: &[SubtaskRow]) -> Self {
        Self(
            rows.iter()
                .enumerate()
                .filter_map(|(row, subtask)| Some((row, subtask.category()?.to_owned())))
                .collect(),
        )
    }
    /// Fly every token from its segment to its category's place in the band.
    fn launch(&self, workspace: &web_sys::Element, hues: &CategoryHues) {
        let Some(layer) = workspace.query_selector(".curation-flights").ok().flatten() else {
            return;
        };
        let find = |selector: String| workspace.query_selector(&selector).ok().flatten();
        for (row, category) in &self.0 {
            let (Some(segment), Some(to)) = (
                find(format!(".curation-map-segment[data-row=\"{row}\"]")),
                hues.rank(category)
                    .and_then(|rank| find(format!("[data-bin=\"{rank}\"]")))
                    .map(|bin| Place::of(&bin).center()),
            ) else {
                continue;
            };
            let from = Place::of(&segment).center();
            // Each row arcs up into its bin.
            let via = ((from.0 + to.0) / 2.0, from.1.min(to.1) - 48.0);
            let delay = segment
                .get_attribute("data-delay")
                .and_then(|delay| delay.parse::<f64>().ok())
                .unwrap_or_default();
            Flight::arc(from, via, to)
                .after(delay)
                .token("curation-flight-token", &hues.tint_of(Some(category)))
                .launch(&layer);
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
    /// Its place in the band and the wash it wears.
    rank: usize,
    /// Which episodes those members came from, out of every episode the
    /// annotation measured.
    coverage: EpisodeCoverage,
    representatives: Vec<String>,
    /// Whether the grouping hands its members on.
    kept: bool,
}

impl CategorySummary {
    /// Every category's card, biggest first, over the rows the node judged,
    /// each saying whether it is among the `kept`.
    fn all(
        categories: &[SubtaskCategory],
        kept: &[String],
        rows: &[SubtaskRow],
        source: &SubtaskSource,
    ) -> Vec<Self> {
        let hues = CategoryHues::of_rows(categories, rows);
        let mut cards = categories
            .iter()
            .map(|category| {
                let mut members = rows
                    .iter()
                    .enumerate()
                    .filter_map(|(row, subtask)| {
                        let placed = subtask.placement()?;
                        (placed.category() == category.name()).then(|| (row, placed.probability()))
                    })
                    .collect::<Vec<_>>();
                members.sort_by(|left, right| right.1.total_cmp(&left.1));
                Self {
                    category: category.clone(),
                    share: if rows.is_empty() {
                        0.0
                    } else {
                        members.len() as f64 / rows.len() as f64
                    },
                    rank: hues.rank(category.name()).unwrap_or_default(),
                    coverage: EpisodeCoverage::of(
                        &source.measured(),
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
                    kept: kept.iter().any(|kept| kept == category.name()),
                }
            })
            .collect::<Vec<_>>();
        cards.sort_by_key(|card| card.rank);
        cards
    }
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
        format!("--hue: {}", self.colour())
    }
    fn colour(&self) -> String {
        format!(
            "var(--tone-category-{})",
            CategoryHues::tone_of_rank(self.rank)
        )
    }
}

/// The workspace shown for a selected Group similar subtasks node.
#[component]
pub(in crate::activities::curation) fn GroupingWorkspace(
    controller: CurationController,
    node: String,
) -> Element {
    let grouping = GroupingNode {
        id: node.clone(),
        controller,
    };
    let source = SubtaskSource::of(controller, &node);
    let categories = grouping.categories();
    let spans = controller.sample().as_ref().map(EpisodeSpans::of);
    // The categories are being proposed, or why proposing or running failed.
    let mut proposing = use_signal(|| false);
    let mut problem = use_signal(|| Option::<String>::None);
    let mut opened = use_signal(|| Option::<CategoryEdit>::None);
    let run = controller.run_state(&node);
    let running = run != RunState::Idle;
    let busy = proposing() || running;
    // The members a click opens, one tab per episode; the row chosen is the
    // one playing in the tab in front.
    let tabs = OpenEpisodes::use_open();
    let rows = source.judged().to_vec();
    let keys = RowKeys::of(&rows);
    let open_row = {
        let rows = rows.clone();
        move |row: usize| {
            if let Some(row) = rows.get(row) {
                tabs.open(row.episode(), Some((row.start_time(), row.end_time())));
            }
        }
    };
    let mut reveal = use_signal(|| MapReveal::Still);
    let mut generation = use_signal(|| 0_u32);
    let mut workspace = use_signal(|| None::<web_sys::Element>);
    let kept = grouping.kept();
    let cards = CategorySummary::all(&categories, &kept, &rows, &source);
    let hues = CategoryHues::of_rows(&categories, &rows);

    // A grouping with nothing open is a band of cards over an empty half-pane,
    // which says nothing about the rows it just sorted. Arriving on one — a new
    // node, a new list, or the first rows landing — opens its biggest category
    // on its most typical member. Closing it afterwards stays closed.
    let biggest = cards
        .first()
        .filter(|card| !card.members.is_empty())
        .map(|card| (card.category.clone(), card.most_typical()));
    let landed = !rows.is_empty();
    let opening = open_row.clone();
    use_effect(use_reactive((&categories, &landed), move |(_, landed)| {
        let Some((category, typical)) = biggest.clone().filter(|_| landed) else {
            opened.set(None);
            return;
        };
        opened.set(Some(CategoryEdit::of(&category)));
        if let Some(row) = typical {
            opening(row);
        }
    }));
    // A run that finishes while the grouping is on screen pours its rows from
    // the map into their categories.
    let mut was_running = use_signal(|| false);
    let poured = node.clone();
    use_effect(use_reactive((&running,), move |(running,)| {
        let finished = *was_running.peek() && !running;
        was_running.set(running);
        if !finished {
            return;
        }
        reveal.set(MapReveal::Pour);
        generation += 1;
        let poured = poured.clone();
        spawn(async move {
            Motion::after_render().await;
            let source = SubtaskSource::of(controller, &poured);
            let rows = source.judged();
            let categories = GroupingNode {
                id: poured.clone(),
                controller,
            }
            .categories();
            if let Some(workspace) = workspace.peek().as_ref() {
                PourPlan::of(rows).launch(workspace, &CategoryHues::of_rows(&categories, rows));
            }
        });
    }));
    // Proposing takes a few seconds, so the card and the top bar say so; a
    // run says what it is doing itself. Proposing belongs to this workspace,
    // so leaving the node ends it and the line with it.
    let reported = node.clone();
    use_effect(use_reactive((&proposing(),), move |(proposing,)| {
        if proposing {
            controller.report_activity(&reported, Some("Proposing categories".to_owned()));
        } else if controller.run_state(&reported) == RunState::Idle {
            controller.report_activity(&reported, None);
        }
    }));
    // Grouping costs one judgment of each subtask, so categories the node has
    // not grouped the subtasks that reach it now by are grouped at once: on
    // opening a grouping whose answer is gone or out of date. A grouping
    // whose last run failed waits for Run.
    let regroup = grouping.clone();
    use_effect(move || {
        let Some(progress) = controller.progress(&regroup.id) else {
            return;
        };
        if !regroup.categories().is_empty()
            && !progress.pending().is_empty()
            && progress.failed().is_empty()
            && !*proposing.peek()
            && controller.run_state(&regroup.id) == RunState::Idle
        {
            problem.set(regroup.run().err());
        }
    });
    let dropped = node.clone();
    use_drop(move || {
        if controller.run_state(&dropped) == RunState::Idle {
            controller.report_activity(&dropped, None);
        }
    });

    // Propose categories from the subtasks that reach the node, then classify
    // every episode by them.
    let propose = {
        let grouping = grouping.clone();
        let source = source.clone();
        use_callback(move |()| {
            if *proposing.peek() || controller.run_state(&grouping.id) != RunState::Idle {
                return;
            }
            problem.set(None);
            let descriptions = source
                .reaching()
                .iter()
                .map(|row| row.description().to_owned())
                .collect::<Vec<_>>();
            if descriptions.is_empty() {
                problem.set(Some(
                    "Run Break down subtasks first; grouping reads the subtasks its preview read."
                        .to_owned(),
                ));
                return;
            }
            let grouping = grouping.clone();
            proposing.set(true);
            reveal.set(MapReveal::Reading);
            spawn(async move {
                let proposed = GroupingNode::propose(descriptions).await;
                proposing.set(false);
                reveal.set(MapReveal::Still);
                problem.set(
                    proposed
                        .and_then(|categories| grouping.regroup(Regroup::Proposed(categories)))
                        .err(),
                );
            });
        })
    };
    // A grouping has nothing to show before it has categories, so it
    // proposes them itself once the subtasks that reach it have all arrived:
    // Break down has described some and no node above is still running. A
    // proposal that failed waits for Try again.
    let unproposed = grouping.clone();
    use_effect(move || {
        let arrived = controller
            .populations(&unproposed.id)
            .is_some_and(|(input, _)| !input.subtasks().is_empty());
        if arrived
            && unproposed.categories().is_empty()
            && !unproposed.subtasks_arriving()
            && problem.peek().is_none()
            && !*proposing.peek()
        {
            propose.call(());
        }
    });

    let Some(annotation) = source.annotation().map(str::to_owned) else {
        return rsx! {
            section { class: "curation-similar-workspace",
                Notice { tone: NoticeTone::Danger,
                    "This grouping is not connected to a Break down subtasks node."
                }
            }
        };
    };
    let reaching = source.reaching().len();
    let context = source.context();
    let measured = source.measured();
    let failed = source.progress().failed().len();
    let reason = source
        .progress()
        .failed()
        .first()
        .map(|(_, reason)| reason.clone());
    let stale = source
        .progress()
        .is_stale()
        .then(|| "Out of date: its subtasks changed".to_owned());
    let pending = source.progress().pending().len();
    let open = opened.read().clone();
    let open_card = open
        .as_ref()
        .and_then(|edit| {
            cards
                .iter()
                .find(|card| card.category.name() == edit.original())
        })
        .cloned();
    let open_name = open_card
        .as_ref()
        .map(|card| card.category.name().to_owned());
    let thumbnails = Thumbnails::use_camera(controller, context.camera());
    let chosen = tabs.front().and_then(|tab| {
        rows.iter()
            .position(|row| tab.holds(row.episode(), row.start_time()))
    });
    // An open category needs the height for its members and their footage,
    // so the band steps back to one row of chips.
    let collapsed = open_card.is_some();
    let slices = rows.iter().map(Slice::of_subtask).collect::<Vec<_>>();
    let lanes = spans
        .as_ref()
        .map(|spans| EpisodeLanes::of(measured.episodes(), &slices, spans))
        .unwrap_or_default();
    // An open category stands out from the rest; otherwise the categories
    // the node leaves out step back.
    let paint = MapPaint::new(
        rows.iter()
            .map(|row| match row.category() {
                Some(category) => SegmentPaint::Category {
                    hue: hues.hue(category).unwrap_or_default(),
                    faded: match open_name.as_deref() {
                        Some(open) => open != category,
                        None => !kept.iter().any(|kept| kept == category),
                    },
                },
                None => SegmentPaint::Unasked,
            })
            .collect(),
        chosen,
    )
    .revealing(reveal(), generation());
    let summary = match &open_card {
        // The open category is named by its chip and its heading; the map
        // says only how far it reaches.
        Some(card) => card.coverage.summary(),
        None => format!(
            "{} across {}",
            Counted::subtasks(rows.len()),
            Counted::episodes(measured.len())
        ),
    };
    let status = if proposing() {
        Some("Reading the subtask descriptions…".to_owned())
    } else {
        running
            .then(|| controller.activity().get(&node).cloned())
            .flatten()
    };
    let profile = open_name.as_deref().map(|name| {
        let joined = rows.iter().map(SubtaskRow::category).collect::<Vec<_>>();
        CategoryProfile::of(name, &rows, &joined, &spans.clone().unwrap_or_default())
    });
    // Choosing a segment of another category opens that category, so the row
    // is always one of the members listed beside it.
    let jump = {
        let rows = rows.clone();
        let cards = cards.clone();
        let open_row = open_row.clone();
        move |row: usize| {
            let Some(card) = rows
                .get(row)
                .and_then(SubtaskRow::category)
                .and_then(|category| {
                    cards
                        .iter()
                        .find(|card| card.category.name() == category)
                        .cloned()
                })
            else {
                return;
            };
            let place = card.members.iter().position(|(member, _)| *member == row);
            if opened.peek().as_ref().map(CategoryEdit::original) != Some(card.category.name()) {
                opened.set(Some(CategoryEdit::of(&card.category)));
            }
            open_row(row);
            spawn(async move {
                Motion::after_render().await;
                if let Some(place) = place {
                    SubtaskList::MEMBERS.reveal(place);
                }
            });
        }
    };
    let find = {
        let annotation = annotation.clone();
        move |row: SubtaskRow, from: Option<Place>| controller.find_similar(&annotation, &row, from)
    };
    let tint = open_card
        .as_ref()
        .map(CategorySummary::tint)
        .unwrap_or_default();
    let members = open_card
        .as_ref()
        .map(|card| card.members.clone())
        .unwrap_or_default();
    let chosen_place = chosen.and_then(|chosen| members.iter().position(|(row, _)| *row == chosen));
    let colour = open_card
        .as_ref()
        .map(CategorySummary::colour)
        .unwrap_or_default();
    // What the node hands on: the rows of the categories it keeps.
    let kept_rows = cards
        .iter()
        .filter(|card| card.kept)
        .map(|card| card.members.len())
        .sum::<usize>();
    rsx! {
        section {
            class: "curation-similar-workspace curation-similar-grouping",
            "aria-label": "Group similar subtasks",
            onmounted: move |event: Event<MountedData>| {
                workspace.set(event.data().downcast::<web_sys::Element>().cloned());
            },
            onpointerdown: move |_| {
                if let Some(workspace) = workspace.peek().as_ref() {
                    Motion::finish_within(workspace);
                }
            },
            div { class: "curation-flights", "aria-hidden": "true" }
            if !cards.is_empty() {
                // How far a proposal or a run has got, and the ways to run
                // it; at rest the bar holds only what the grouping keeps.
                // The categories are the band below and their count is on
                // the card.
                NodeBar {
                    lead: rsx! {
                        p { class: "curation-node-bar-reads", role: "status",
                            if let Some(status) = status.clone() { "{status}" }
                        }
                    },
                    note: NodeNotes::join([stale, FailedEpisodes::note(failed)]),
                    reach: NodeReach::of(controller, &node).map(|reach| (controller, reach)),
                    actions: rsx! {
                        if !rows.is_empty() {
                            p { class: "curation-threshold-tally", role: "status",
                                "Keeps {kept_rows} of {Counted::subtasks(rows.len())}"
                            }
                        }
                        if running {
                            Button {
                                purpose: ActionPurpose::Stop, variant: ButtonVariant::Outline,
                                disabled: run == RunState::Stopping,
                                title: "Stop grouping; the episodes in flight stay ungrouped",
                                onclick: {
                                    let node = node.clone();
                                    move |_| controller.stop_run(&node)
                                },
                                if run == RunState::Stopping { "Stopping…" } else { "Stop" }
                            }
                        } else {
                            ConfirmedRerun {
                                id: "curation-similar-regenerate".to_owned(),
                                label: "Regenerate".to_owned(),
                                disabled: busy,
                                question: format!(
                                    "Propose categories again? This replaces the {} categories and groups every subtask again.",
                                    cards.len(),
                                ),
                                on_confirm: move |()| propose.call(()),
                            }
                            if pending + failed > 0 {
                                Button {
                                    purpose: ActionPurpose::Start, variant: ButtonVariant::Solid,
                                    disabled: busy,
                                    title: "Group the subtasks of {Counted::episodes(pending)} by these categories",
                                    onclick: {
                                        let grouping = grouping.clone();
                                        move |_| problem.set(grouping.run().err())
                                    },
                                    if pending + failed < measured.len() { "Run {pending + failed} left" } else { "Run" }
                                }
                            }
                        }
                    },
                }
            }
            // The dataset first, then the bins its rows pour down into, then
            // the open bin's members: each band reads as the one above it,
            // sorted.
            if !rows.is_empty() {
                div { class: "curation-similar-overview",
                    EpisodeMap { lanes, paint, summary, on_select: jump }
                }
            }
            // Before it has categories a grouping says only what it waits on,
            // with the action that ends the wait when there is one.
            if cards.is_empty() && reaching == 0 && !grouping.subtasks_arriving() {
                EmptyState { title: "No subtasks to group",
                    Button {
                        purpose: ActionPurpose::Navigate,
                        variant: ButtonVariant::Outline,
                        onclick: {
                            let annotation = annotation.clone();
                            move |_| controller.select_node(Some(annotation.clone()))
                        },
                        "Open Break down subtasks"
                    }
                }
            } else if cards.is_empty() {
                EmptyState { title: "No categories yet",
                    p { role: "status",
                        match status.clone() {
                            Some(status) => rsx! { "{status}" },
                            None if grouping.subtasks_arriving() => rsx! {
                                "{Counted::subtasks(reaching)} so far, proposed once the steps above finish"
                            },
                            None => rsx! {},
                        }
                    }
                    if problem().is_some() && !busy {
                        Button {
                            purpose: ActionPurpose::Start,
                            variant: ButtonVariant::Solid,
                            onclick: move |_| propose.call(()),
                            "Try again"
                        }
                    }
                }
            } else {
                div {
                    class: if collapsed { "curation-similar-band curation-similar-chips" } else { "curation-similar-band curation-similar-categories" },
                    for summary in cards.clone() {
                        CategoryCard {
                            key: "{summary.category.name()}",
                            summary: summary.clone(),
                            face: if collapsed {
                                CategoryFace::Chip {
                                    open: open.as_ref().map(CategoryEdit::original) == Some(summary.category.name()),
                                }
                            } else {
                                CategoryFace::Card
                            },
                            on_keep: {
                                let grouping = grouping.clone();
                                let name = summary.category.name().to_owned();
                                EventHandler::new(move |keep: bool| {
                                    problem.set(grouping.keep(&name, keep).err());
                                })
                            },
                            on_open: {
                                let open_row = open_row.clone();
                                EventHandler::new(move |()| {
                                    let same = opened.peek().as_ref().map(CategoryEdit::original)
                                        == Some(summary.category.name());
                                    opened.set((!same).then(|| CategoryEdit::of(&summary.category)));
                                    // Opening a category lands on its most typical
                                    // member, so the evidence column is never an
                                    // empty half beside a full list.
                                    if let Some(row) = (!same).then(|| summary.most_typical()).flatten() {
                                        open_row(row);
                                    }
                                })
                            },
                        }
                    }
                }
            }
            if let Some(problem) = problem() {
                Notice { tone: NoticeTone::Danger, "{problem}" }
            }
            if let Some(reason) = reason {
                Notice { tone: NoticeTone::Danger, "An episode failed: {reason}" }
            }
            if let (Some(edit), Some(profile)) = (open, profile) {
                SubtaskReview {
                    index: rsx! {
                        div { class: "curation-similar-open-category", style: "{tint}",
                            div { class: "curation-similar-open",
                                h3 { class: "curation-similar-open-name",
                                    "{edit.original()}"
                                    span { class: "curation-similar-open-definition", "{edit.definition()}" }
                                }
                                Disclosure { summary: rsx! { "Rename or merge" },
                                    CategoryDetail {
                                        edit: edit.clone(),
                                        grouping: grouping.clone(),
                                        categories: categories.clone(),
                                        busy,
                                    }
                                }
                            }
                            ol {
                                id: SubtaskList::MEMBERS.id(),
                                class: "curation-entries",
                                "aria-label": "Members of this category, most typical first; each row ends with how sure it is that the row belongs here",
                                for (position , (row , probability)) in members.into_iter().enumerate() {
                                    SubtaskEntry {
                                        key: "{keys[row]}",
                                        row: rows[row].clone(),
                                        standing: RowStanding::scored(probability, true, chosen == Some(row))
                                            .roving(TabStop::at(position, chosen_place)),
                                        episode: Episode::listed_name_of(rows[row].episode()),
                                        thumbnails,
                                        on_select: {
                                            let open_row = open_row.clone();
                                            move |()| open_row(row)
                                        },
                                        on_find_similar: {
                                            let find = find.clone();
                                            let asked = rows[row].clone();
                                            move |from| find(asked.clone(), from)
                                        },
                                    }
                                }
                            }
                        }
                    },
                    evidence: rsx! {
                        EpisodeTabs {
                            open: tabs,
                            // How the open category differs from the rest, a tab
                            // beside its members' recordings rather than a wall
                            // above the members themselves.
                            leading: LeadingTab::new("Profile", 'C', colour.clone(), rsx! {
                                ProfilePanel {
                                    profile,
                                    hues: hues.clone(),
                                    chosen,
                                    on_select: {
                                        let open_row = open_row.clone();
                                        move |row: usize| open_row(row)
                                    },
                                }
                            }),
                            render: move |tab: OpenEpisode| {
                                let Some(row) = rows
                                    .iter()
                                    .find(|row| tab.holds(row.episode(), row.start_time()))
                                    .cloned()
                                else {
                                    return rsx! {};
                                };
                                rsx! {
                                    SubtaskEvidence {
                                        play: tab.play(),
                                        // Each subtask in the hue of the category it joined,
                                        // as the map above paints it.
                                        intervals: SubtaskIntervals::painted(
                                            &SubtaskIntervals::episode_of(&row, &rows),
                                            |other| hues.tint_of(other.category()),
                                        )
                                        .choosing(Some(&row)),
                                        context: context.clone(),
                                        on_open: {
                                            let focus = EpisodeFocus::new(
                                                annotation.clone(),
                                                row.episode().clone(),
                                                Some((row.start_time(), row.end_time())),
                                            );
                                            move |()| controller.focus_episode(focus.clone())
                                        },
                                        on_find_similar: {
                                            let find = find.clone();
                                            let asked = row.clone();
                                            EventHandler::new(move |()| find(asked.clone(), None))
                                        },
                                        row,
                                    }
                                }
                            },
                        }
                    },
                }
            }
        }
    }
}

/// How a category is drawn: a card in the band while no category is open, or
/// a chip in one row once one is, because the open category owns the height.
#[derive(Clone, Copy, Debug, PartialEq)]
enum CategoryFace {
    Card,
    Chip { open: bool },
}

/// One category: whether the grouping keeps it, what it is, how many rows
/// joined it, which episodes they came from, and the rows that joined it
/// most typically.
///
/// While a category is open the band has no height to spare for six of these,
/// so every card steps back to its name and its counts alone.
#[component]
fn CategoryCard(
    summary: CategorySummary,
    face: CategoryFace,
    on_open: EventHandler<()>,
    /// The category kept (`true`) or left out of what the node hands on.
    on_keep: EventHandler<bool>,
) -> Element {
    let share = format!("{:.0}%", summary.share * 100.0);
    let counted = summary.counted();
    let keep = rsx! {
        KeepCheck { name: summary.category.name().to_owned(), kept: summary.kept, on_keep }
    };
    let CategoryFace::Chip { open } = face else {
        return rsx! {
            // The examples are a list, and a list may not live inside a
            // button, so the button is the card's heading and the card is the
            // article.
            article {
                class: "curation-similar-category",
                style: "{summary.tint()}",
                "data-kept": "{summary.kept}",
                "data-bin": "{summary.rank}",
                div { class: "curation-similar-category-head",
                    {keep}
                    button {
                        r#type: "button",
                        class: "curation-similar-category-open",
                        "aria-expanded": "false",
                        onclick: move |_| on_open.call(()),
                        span { class: "curation-similar-category-name", "{summary.category.name()}" }
                    }
                }
                div {
                    class: "curation-similar-category-share",
                    "aria-hidden": "true",
                    span { style: "width:{summary.share * 100.0}%" }
                }
                // The share the bar draws, the rows it counted, and the
                // episodes they came from — the bar is never the only place
                // the number is.
                p { class: "curation-similar-category-count", "{share} · {counted}" }
                p { class: "curation-similar-category-definition", "{summary.category.definition()}" }
                ul { class: "curation-similar-category-examples",
                    for (place, representative) in summary.representatives.iter().enumerate() {
                        li { key: "{place}", "{representative}" }
                    }
                }
            }
        };
    };
    // The chip carries the name and the size; the open category below it is
    // already saying which episodes it reached, and six names plus six pairs
    // of counts would cost the rows this band just gave back.
    let members = Counted::subtasks(summary.members.len());
    rsx! {
        div {
            class: "curation-similar-chip",
            style: "{summary.tint()}",
            "data-selected": "{open}",
            "data-kept": "{summary.kept}",
            "data-bin": "{summary.rank}",
            {keep}
            button {
                r#type: "button",
                class: "curation-similar-chip-open",
                title: "{members}",
                "aria-label": "{summary.category.name()}, {members}",
                "aria-expanded": "{open}",
                onclick: move |_| on_open.call(()),
                span { class: "curation-similar-chip-name", "{summary.category.name()}" }
                span { class: "curation-similar-chip-count", "{summary.members.len()}" }
            }
        }
    }
}

/// Whether a category's subtasks are part of what the grouping hands on: a
/// checkbox before its name, on its card and on its chip alike.
#[component]
fn KeepCheck(name: String, kept: bool, on_keep: EventHandler<bool>) -> Element {
    rsx! {
        input {
            r#type: "checkbox",
            class: "curation-similar-keep",
            checked: kept,
            title: if kept { "Kept in this output; uncheck to leave it out" } else { "Left out of this output; check to keep it" },
            "aria-label": "Keep {name}",
            onchange: move |event| on_keep.call(event.checked()),
        }
    }
}

/// The open category's name, definition, and the merge that removes it.
/// Both actions run only the classification again.
#[component]
fn CategoryDetail(
    edit: CategoryEdit,
    grouping: GroupingNode,
    categories: Vec<SubtaskCategory>,
    busy: bool,
) -> Element {
    let mut draft = use_signal(|| edit.clone());
    let mut problem = use_signal(|| Option::<String>::None);
    let mut target = use_signal(String::new);
    use_effect(use_reactive(&edit, move |edit| {
        draft.set(edit);
        problem.set(None);
        target.set(String::new());
    }));
    let apply = use_callback(move |change: Result<Regroup, String>| {
        problem.set(change.and_then(|change| grouping.regroup(change)).err());
    });
    let others = categories
        .iter()
        .filter(|category| category.name() != edit.original())
        .map(|category| category.name().to_owned())
        .collect::<Vec<_>>();
    let chosen = target();
    rsx! {
        div { class: "curation-similar-editor",
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
                onclick: move |_| {
                    let draft = draft.read();
                    apply.call(
                        SubtaskCategory::new(draft.name(), draft.definition()).map(|edited| {
                            Regroup::Edited {
                                original: draft.original().to_owned(),
                                edited,
                            }
                        }),
                    );
                },
                "Apply and regroup"
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
                    id: "curation-similar-merge".to_owned(),
                    label: "Merge".to_owned(),
                    disabled: busy || chosen.is_empty(),
                    question: format!(
                        "Merge “{}” into “{chosen}”? Every subtask is grouped again and the category is gone.",
                        edit.original(),
                    ),
                    on_confirm: move |()| {
                        apply.call(Ok(Regroup::Merged {
                            merged: draft.peek().original().to_owned(),
                            into: chosen.clone(),
                        }));
                    },
                }
            }
            if let Some(problem) = problem() {
                Notice { tone: NoticeTone::Danger, "{problem}" }
            }
        }
    }
}
