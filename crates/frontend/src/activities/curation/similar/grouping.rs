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
use super::coverage::{CoverageMeasure, EpisodeCoverage, SubtaskList};
use super::decisions::DecisionRun;
use super::profile::{CategoryProfile, ProfilePanel};
use super::questions::AssignmentQuestions;
use super::source::SubtaskSource;
use crate::activities::curation::document::{
    CurationNodeKind, DEFAULT_ANNOTATION_MODEL, SubtaskAssignment, SubtaskCategory,
    SubtaskGrouping, SubtaskRow,
};
use crate::activities::curation::episode_map::{
    EpisodeLanes, EpisodeMap, EpisodeSpans, MapPaint, MapReveal, SegmentPaint,
};
use crate::activities::curation::episode_tabs::{
    EpisodeTabs, LeadingTab, OpenEpisode, OpenEpisodes,
};
use crate::activities::curation::population::Slice;
use crate::activities::curation::poster::{PosterCut, PosterFrames};
use crate::activities::curation::rerun::ConfirmedRerun;
use crate::activities::curation::review::{
    Counted, NodeBar, NodeReach, RowStanding, SubtaskEntry, SubtaskEvidence, SubtaskIntervals,
    SubtaskReview, TabStop,
};
use crate::activities::curation::state::{CurationController, EpisodeFocus};
use crate::activities::episode::Episode;
use crate::client::completions::{ModelClient, ModelRequest};
use crate::shell::state::StudioProjectState;
use dioxus::prelude::*;
use spiral_robotics_common::providers::ProviderMessage;
use spiral_robotics_ui::{
    ActionPurpose, Arrangement, Button, ButtonVariant, Disclosure, EmptyState, Field, FieldLayout,
    Flight, Motion, Notice, NoticeTone, Place, Select, TextInput,
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
    /// The line the node's card and the top bar carry while the grouping runs.
    fn activity(&self) -> Option<String> {
        match self {
            Self::Proposing => Some("Proposing categories".to_owned()),
            Self::Assigning { answered, requests } => {
                Some(format!("Grouping · {answered} of {requests} batches"))
            }
            Self::Failed(_) => None,
        }
    }
}

/// Which colour, and which place in the band, each category gets: biggest
/// first, the order an operator reads a taxonomy in, so the hue a category
/// wears means its size rather than the order the model happened to name it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(in crate::activities::curation) struct CategoryHues(Vec<String>);

impl CategoryHues {
    fn of(grouping: &SubtaskGrouping, source: &SubtaskSource) -> Self {
        Self::of_rows(grouping, source.rows())
    }
    /// The hues of a grouping's categories, ranked by how many of `rows` each
    /// holds, so a Select after the grouping wears the grouping's colours.
    pub(in crate::activities::curation) fn of_rows(
        grouping: &SubtaskGrouping,
        rows: &[SubtaskRow],
    ) -> Self {
        let mut sized = grouping
            .categories()
            .iter()
            .map(|category| {
                let members = rows
                    .iter()
                    .filter(|row| grouping.category_of(row) == Some(category.name()))
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
    pub(in crate::activities::curation) fn tint_of(&self, category: Option<&str>) -> String {
        category
            .and_then(|category| self.hue(category))
            .map(|hue| format!("--hue: var(--tone-category-{})", hue + 1))
            .unwrap_or_default()
    }
}

/// Below this confidence jev was torn between categories, and the row says so:
/// its flight bends toward the runner-up and its segment keeps a hatched edge.
const HESITANT_BELOW: f64 = 0.9;

/// One row leaving the map for its category.
#[derive(Clone, Debug, PartialEq)]
struct PourToken {
    row: usize,
    category: String,
    /// The runner-up a hesitant row bends toward on the way.
    detour: Option<String>,
}

/// The rows a landed grouping sends flying: every row of a first grouping, and
/// only the rows whose category changed when an existing one was reassigned —
/// the motion is the difference the run made.
#[derive(Clone, Debug, Default, PartialEq)]
struct PourPlan(Vec<PourToken>);

impl PourPlan {
    fn of(
        before: Option<&SubtaskGrouping>,
        after: &SubtaskGrouping,
        source: &SubtaskSource,
    ) -> Self {
        let earlier = before.map(|before| source.assignments(before));
        Self(
            source
                .assignments(after)
                .into_iter()
                .enumerate()
                .filter_map(|(row, assignment)| {
                    let assignment = assignment?;
                    let unchanged = earlier
                        .as_ref()
                        .and_then(|earlier| earlier[row])
                        .is_some_and(|earlier| earlier.category() == assignment.category());
                    (!unchanged).then(|| PourToken {
                        row,
                        category: assignment.category().to_owned(),
                        detour: (assignment.confidence() < HESITANT_BELOW)
                            .then(|| {
                                assignment
                                    .runner_up()
                                    .map(|runner| runner.category().to_owned())
                            })
                            .flatten(),
                    })
                })
                .collect(),
        )
    }
    /// Fly every token from its segment to its category's place in the band.
    fn launch(&self, workspace: &web_sys::Element, hues: &CategoryHues) {
        let Some(layer) = workspace.query_selector(".curation-flights").ok().flatten() else {
            return;
        };
        let find = |selector: String| workspace.query_selector(&selector).ok().flatten();
        let bin = |category: &str| {
            hues.rank(category)
                .and_then(|rank| find(format!("[data-bin=\"{rank}\"]")))
                .map(|bin| Place::of(&bin).center())
        };
        for token in &self.0 {
            let (Some(segment), Some(to)) = (
                find(format!(".curation-map-segment[data-row=\"{}\"]", token.row)),
                bin(&token.category),
            ) else {
                continue;
            };
            let from = Place::of(&segment).center();
            let middle = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
            // A sure row arcs straight up into its bin; a torn one is pulled
            // most of the way toward the bin it nearly joined first.
            let via = match token.detour.as_deref().and_then(bin) {
                Some(detour) => (
                    middle.0 + 0.8 * (detour.0 - middle.0),
                    middle.1 + 0.8 * (detour.1 - middle.1),
                ),
                None => (middle.0, from.1.min(to.1) - 48.0),
            };
            let delay = segment
                .get_attribute("data-delay")
                .and_then(|delay| delay.parse::<f64>().ok())
                .unwrap_or_default();
            Flight::arc(from, via, to)
                .after(delay)
                .token(
                    "curation-flight-token",
                    &hues.tint_of(Some(&token.category)),
                )
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
    let studio = StudioProjectState::current();
    let source = use_memo(use_reactive((&node,), move |(node,)| {
        SubtaskSource::of(controller, &node)
    }));
    let saved = use_memo(use_reactive((&node,), move |(node,)| {
        controller.with_payload(|payload| payload.grouping_for(&node).cloned())
    }));
    let spans = controller.sample().as_ref().map(EpisodeSpans::of);
    let mut stage = use_signal(|| Option::<GroupingStage>::None);
    // The categories the model proposed, shown as empty bins while jev fills
    // them.
    let mut pending = use_signal(|| Option::<Vec<SubtaskCategory>>::None);
    let mut opened = use_signal(|| Option::<CategoryEdit>::None);
    // The members a click opens, one tab per episode; the row chosen is the
    // one playing in the tab in front.
    let tabs = OpenEpisodes::use_open();
    let open_row = move |row: usize| {
        if let Some(row) = source.peek().rows().get(row) {
            tabs.open(row.episode(), Some((row.start_time(), row.end_time())));
        }
    };
    let mut reveal = use_signal(|| MapReveal::Still);
    let mut generation = use_signal(|| 0_u32);
    let mut workspace = use_signal(|| None::<web_sys::Element>);
    // A grouping with nothing open is a band of cards over an empty half-pane,
    // which says nothing about the rows it just sorted. Arriving on one — a new
    // node, or a run that just finished — opens its biggest category on its
    // most typical member. Closing it afterwards stays closed. The rows come
    // with the episode sample, so a node opened before it arrives waits for
    // them rather than opening a category of nothing.
    let row_count = source.read().rows().len();
    use_effect(use_reactive(
        (&saved(), &row_count),
        move |(grouping, rows)| {
            if rows == 0 {
                return;
            }
            let biggest = grouping.as_ref().and_then(|grouping| {
                let reading = source.peek();
                let hues = CategoryHues::of(grouping, &reading);
                let name = hues.0.first()?.clone();
                let category = grouping
                    .categories()
                    .iter()
                    .find(|category| category.name() == name)?;
                Some((category.clone(), reading.listed(&grouping.members(&name))))
            });
            match biggest {
                Some((category, members)) => {
                    opened.set(Some(CategoryEdit::of(&category)));
                    if let Some((row, _)) = members.first() {
                        open_row(*row);
                    }
                }
                None => opened.set(None),
            }
        },
    ));

    // A grouping runs for a minute or more, so its card and the top bar say
    // so. Its requests belong to this workspace, so leaving the node ends
    // them and the line with them.
    let reported = node.clone();
    let activity = stage.read().as_ref().and_then(GroupingStage::activity);
    use_effect(use_reactive((&activity,), move |(activity,)| {
        controller.report_activity(&reported, activity);
    }));
    let dropped = node.clone();
    use_drop(move || controller.report_activity(&dropped, None));
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
                    Some(CurationNodeKind::BreakDownSubtasks(annotation)) => {
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
        reveal.set(MapReveal::Reading);
        spawn(async move {
            let categories = match categories {
                Some(categories) => categories,
                None => match pass.propose().await {
                    Ok(categories) => categories,
                    Err(problem) => {
                        reveal.set(MapReveal::Still);
                        stage.set(Some(GroupingStage::Failed(problem)));
                        return;
                    }
                },
            };
            pending.set(Some(categories.clone()));
            let assigned = pass
                .assign(categories, |answered, requests| {
                    stage.set(Some(GroupingStage::Assigning { answered, requests }));
                })
                .await;
            // Where the bins stand before they re-sort, and what each row
            // belonged to before this run, so the landing can show both
            // changing.
            let band = workspace.peek().as_ref().and_then(|workspace| {
                workspace
                    .query_selector(".curation-similar-band")
                    .ok()
                    .flatten()
            });
            let arranged = band.as_ref().map(Arrangement::measure);
            let before = saved.peek().clone();
            match assigned.and_then(|grouping| {
                controller.edit(|payload| payload.set_grouping(grouping.clone()))?;
                Ok(grouping)
            }) {
                Ok(grouping) => {
                    controller.save(studio);
                    stage.set(None);
                    pending.set(None);
                    reveal.set(MapReveal::Pour);
                    generation += 1;
                    let plan = PourPlan::of(before.as_ref(), &grouping, &source.peek());
                    Motion::after_render().await;
                    if let (Some(band), Some(arranged)) = (band, arranged) {
                        arranged.settle(&band, 40.0);
                    }
                    if let Some(workspace) = workspace.peek().as_ref() {
                        plan.launch(workspace, &CategoryHues::of(&grouping, &source.peek()));
                    }
                }
                Err(problem) => {
                    reveal.set(MapReveal::Still);
                    pending.set(None);
                    stage.set(Some(GroupingStage::Failed(problem)));
                }
            }
        });
    });

    let reading = source.read();
    if reading.annotation().is_none() {
        return rsx! {
            section { class: "curation-similar-workspace",
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
    let hues = grouping
        .as_ref()
        .map(|grouping| CategoryHues::of(grouping, &reading))
        .unwrap_or_default();
    let placed = grouping
        .as_ref()
        .map(|grouping| {
            reading
                .assignments(grouping)
                .into_iter()
                .map(|assignment| assignment.cloned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![None; rows.len()]);
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
                        rank: hues.rank(category.name()).unwrap_or_default(),
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
            cards.sort_by_key(|card| card.rank);
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
    let open_name = open_card
        .as_ref()
        .map(|card| card.category.name().to_owned());
    let frames = PosterFrames::use_frames(PosterCut::of_rows(&context, &rows));
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
    let chosen = tabs.front().and_then(|tab| {
        rows.iter()
            .position(|row| tab.holds(row.episode(), row.start_time()))
    });
    let waiting = pending.read().clone();
    // An open category needs the height for its members and their footage, and
    // bins waiting to be filled have nothing to say but their names, so both
    // step the band back to one row of chips.
    let collapsed = open_card.is_some() || waiting.is_some();
    let slices = rows.iter().map(Slice::of_subtask).collect::<Vec<_>>();
    let lanes = spans
        .as_ref()
        .map(|spans| EpisodeLanes::of(measured.episodes(), &slices, spans))
        .unwrap_or_default();
    let paint = MapPaint::new(
        placed
            .iter()
            .map(|assignment| match assignment {
                Some(assignment) => SegmentPaint::Category {
                    hue: hues.hue(assignment.category()).unwrap_or_default(),
                    hesitant: assignment.confidence() < HESITANT_BELOW,
                    faded: open_name
                        .as_deref()
                        .is_some_and(|open| open != assignment.category()),
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
    let profile = open_name.as_deref().map(|name| {
        let joined = placed
            .iter()
            .map(|assignment| assignment.as_ref().map(SubtaskAssignment::category))
            .collect::<Vec<_>>();
        CategoryProfile::of(name, &rows, &joined, &spans.clone().unwrap_or_default())
    });
    // Choosing a segment of another category opens that category, so the row
    // is always one of the members listed beside it.
    let jump = {
        let placed = placed.clone();
        let cards = cards.clone();
        move |row: usize| {
            let Some(card) = placed.get(row).cloned().flatten().and_then(|assignment| {
                cards
                    .iter()
                    .find(|card| card.category.name() == assignment.category())
                    .cloned()
            }) else {
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
            if !cards.is_empty() || waiting.is_some() {
                // What the grouping reads — the annotation's words, not its
                // pictures — or how far a run has got, and Regenerate. The
                // categories are the chips below and their count is on the card.
                NodeBar {
                    lead: rsx! {
                        p { class: "curation-node-bar-reads", role: "status",
                            match running.clone() {
                                Some(running) => rsx! { "{running}" },
                                None => rsx! { "Groups the words of {Counted::subtasks(rows.len())} by what the robot does" },
                            }
                        }
                    },
                    reach: NodeReach::of(controller, &node).map(|reach| (controller, reach)),
                    actions: rsx! {
                        ConfirmedRerun {
                            id: "curation-similar-regenerate".to_owned(),
                            label: "Regenerate".to_owned(),
                            disabled: busy,
                            question: format!(
                                "Propose categories again? This replaces the {} categories and all {} assignments below.",
                                cards.len(),
                                rows.len(),
                            ),
                            on_confirm: move |()| group.call(None),
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
            if cards.is_empty() && waiting.is_none() {
                EmptyState { title: "No categories yet",
                    p {
                        "One model pass reads the {rows.len()} saved descriptions and proposes four to eight categories, then every subtask is put in one of them."
                    }
                    Button {
                        purpose: ActionPurpose::Start,
                        variant: ButtonVariant::Solid,
                        disabled: busy || rows.is_empty(),
                        onclick: move |_| group.call(None),
                        if busy { "Generating…" } else { "Generate categories" }
                    }
                    if let Some(running) = running.clone() {
                        p { class: "curation-similar-note", role: "status", "{running}" }
                    }
                }
            } else {
                div {
                    class: if collapsed { "curation-similar-band curation-similar-chips" } else { "curation-similar-band curation-similar-categories" },
                    if let Some(waiting) = waiting.clone() {
                        for (rank , category) in waiting.into_iter().enumerate() {
                            span {
                                key: "{category.name()}",
                                class: "curation-similar-chip",
                                "data-pending": "true",
                                "data-bin": "{rank}",
                                style: "--order: {rank}",
                                "data-motion-key": "{category.name()}",
                                span { class: "curation-similar-chip-name", "{category.name()}" }
                            }
                        }
                    } else {
                        for summary in cards.clone() {
                            CategoryCard {
                                key: "{summary.category.name()}",
                                summary: summary.clone(),
                                opened: open.as_ref().map(CategoryEdit::original) == Some(summary.category.name()),
                                collapsed,
                                on_open: EventHandler::new(move |()| {
                                    let same = opened.peek().as_ref().map(CategoryEdit::original)
                                        == Some(summary.category.name());
                                    opened.set((!same).then(|| CategoryEdit::of(&summary.category)));
                                    // Opening a category lands on its most typical
                                    // member, so the evidence column is never an
                                    // empty half beside a full list.
                                    if let Some(row) = (!same).then(|| summary.most_typical()).flatten() {
                                        open_row(row);
                                    }
                                }),
                            }
                        }
                    }
                }
            }
            if let Some(problem) = problem {
                Notice { tone: NoticeTone::Danger, "{problem}" }
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
                                class: "curation-entries",
                                "aria-label": "Members of this category, most typical first; each row ends with how sure it is that the row belongs here",
                                for (position , (row , probability)) in members.into_iter().enumerate() {
                                    SubtaskEntry {
                                        key: "{rows[row].episode()}-{rows[row].start_time()}",
                                        row: rows[row].clone(),
                                        standing: RowStanding::scored(probability, true, chosen == Some(row))
                                            .roving(TabStop::at(position, chosen_place)),
                                        episode: Episode::listed_name_of(rows[row].episode()),
                                        frames,
                                        on_select: move |()| open_row(row),
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
                            empty: "Choose a member to watch it play, most typical first.".to_owned(),
                            // How the open category differs from the rest, a tab
                            // beside its members' recordings rather than a wall
                            // above the members themselves.
                            leading: LeadingTab::new("Profile", 'C', colour.clone(), rsx! {
                                ProfilePanel {
                                    profile,
                                    hues: hues.clone(),
                                    chosen,
                                    on_select: move |row: usize| open_row(row),
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
                                            |other| hues.tint_of(grouping.as_ref().and_then(|grouping| grouping.category_of(other))),
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
                class: "curation-similar-chip",
                style: "{summary.tint()}",
                title: "{members}",
                "aria-label": "{summary.category.name()}, {members}",
                "aria-expanded": "{opened}",
                "data-selected": "{opened}",
                "data-bin": "{summary.rank}",
                "data-motion-key": "{summary.category.name()}",
                onclick: move |_| on_open.call(()),
                span { class: "curation-similar-chip-name", "{summary.category.name()}" }
                span { class: "curation-similar-chip-count", "{summary.members.len()}" }
            }
        };
    }
    rsx! {
        // The examples are a list, and a list may not live inside a button, so
        // the button is the card's heading and the card is the article.
        article {
            class: "curation-similar-category",
            style: "{summary.tint()}",
            "data-selected": "{opened}",
            "data-bin": "{summary.rank}",
            "data-motion-key": "{summary.category.name()}",
            button {
                r#type: "button",
                class: "curation-similar-category-open",
                "aria-expanded": "{opened}",
                onclick: move |_| on_open.call(()),
                span { class: "curation-similar-category-name", "{summary.category.name()}" }
            }
            div {
                class: "curation-similar-category-share",
                "aria-hidden": "true",
                span { style: "width:{summary.share * 100.0}%" }
            }
            // The share the bar draws, the rows it counted, and the episodes
            // they came from — the bar is never the only place the number is.
            p { class: "curation-similar-category-count", "{share} · {counted}" }
            p { class: "curation-similar-category-definition", "{summary.category.definition()}" }
            ul { class: "curation-similar-category-examples",
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
                    id: "curation-similar-merge".to_owned(),
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
