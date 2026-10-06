//! One side of a rule, one item at a time beside its recording: whole
//! episodes, or stretches of moments.
//!
//! Every listing names its items by their place in what the rule judges, the
//! same number the map and the strips use, so choosing an item anywhere opens
//! its episode in a tab beside the list.
use super::RuleView;
use crate::activities::curation::document::{EpisodeFact, Selection};
use crate::activities::curation::episode_tabs::{
    EpisodeTabs, OpenEpisode, OpenEpisodes, WholeEpisodeTab,
};
use crate::activities::curation::population::Population;
use crate::activities::curation::review::{
    IndexBar, RowStanding, SliceEntry, SliceFace, SubtaskReview, TabStop,
};
use crate::activities::curation::score_progress::{
    Moment, MomentEvidence, ProgressRate, ProgressScorer,
};
use crate::activities::curation::state::CurationController;
use crate::activities::curation::thumbnail::Thumbnails;
use crate::activities::episode::{Episode, EpisodeRows};
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::EpisodeId;

/// The items on the listed side, by their place in the input, and the one
/// playing in the tab in front when it is on this side.
struct ListedItems {
    items: Vec<usize>,
    chosen: Option<usize>,
}

impl ListedItems {
    /// The listed side in input order.
    fn of(kept: &[bool], listed_kept: bool, chosen: Option<usize>) -> Self {
        let items = kept
            .iter()
            .enumerate()
            .filter(|(_, kept)| **kept == listed_kept)
            .map(|(item, _)| item)
            .collect::<Vec<_>>();
        let chosen = chosen.filter(|chosen| items.contains(chosen));
        Self { items, chosen }
    }
    /// Where the first listed item plays, which opens by itself so the
    /// recording column is never an empty half beside the list.
    fn first_place(&self, view: &RuleView) -> Option<(EpisodeId, Option<(f64, f64)>)> {
        self.items.first().and_then(|item| view.place(*item))
    }
    /// Where the chosen item is in the listing, for the list's one tab stop.
    fn chosen_position(&self) -> Option<usize> {
        let chosen = self.chosen?;
        self.items.iter().position(|item| *item == chosen)
    }
}

/// One side of the rule, laid out the same way whatever it lists and however
/// much of it there is: the sides over the index, the recording beside it.
#[component]
fn SideReview(
    side: ListedSide,
    /// The Kept and Left out segments, over the index they switch.
    sides: Element,
    /// The rows, or nothing when this side is empty.
    rows: Option<Element>,
    evidence: Element,
) -> Element {
    rsx! {
        SubtaskReview {
            index: rsx! {
                IndexBar { {sides} }
                match rows {
                    Some(rows) => rows,
                    None => rsx! {
                        p { class: "curation-review-note", role: "status", "{side.view.empty_side(side.listed_kept)}" }
                    },
                }
            },
            evidence,
        }
    }
}

/// Which side of a rule a listing shows: the rule, whether it keeps each item
/// it judges, and whether the kept side is the one listed.
#[derive(Clone, PartialEq)]
pub(super) struct ListedSide {
    view: RuleView,
    kept: Vec<bool>,
    listed_kept: bool,
}

impl ListedSide {
    pub(super) fn new(view: RuleView, kept: Vec<bool>, listed_kept: bool) -> Self {
        Self {
            view,
            kept,
            listed_kept,
        }
    }
    /// This side's items in input order.
    fn listed(&self, chosen: Option<usize>) -> ListedItems {
        ListedItems::of(&self.kept, self.listed_kept, chosen)
    }
    /// How many rows this side lists: its items, or for moments the
    /// stretches they run into — a moment is a frame, not a row.
    pub(super) fn rows(&self) -> usize {
        let listed = self.listed(None);
        match &self.view.input {
            Population::Moments(rows) => Stretch::of(rows, &listed.items).len(),
            _ => listed.items.len(),
        }
    }
    /// Where this side's first row plays: an episode whole, or a stretch's
    /// span.
    pub(super) fn opening(&self) -> Option<(EpisodeId, Option<(f64, f64)>)> {
        let listed = self.listed(None);
        match &self.view.input {
            Population::Moments(rows) => Stretch::of(rows, &listed.items)
                .first()
                .map(|stretch| (stretch.episode.clone(), Some(stretch.span))),
            _ => listed.first_place(&self.view),
        }
    }
}

#[component]
pub(super) fn EpisodeListing(
    controller: CurationController,
    side: ListedSide,
    tabs: OpenEpisodes,
    sides: Element,
) -> Element {
    let view = side.view.clone();
    let chosen = tabs.front().and_then(|tab| view.item_in(&tab));
    let listed = side.listed(chosen);
    let first = side.opening();
    // A condition slid past the episode in front leaves it off this side, so
    // the tab moves on to the side's first episode.
    let unlisted = listed.chosen.is_none();
    use_effect(use_reactive(
        (&first, &unlisted),
        move |(first, unlisted)| {
            if unlisted {
                tabs.show_first(first);
            } else {
                tabs.open_first(first);
            }
        },
    ));
    let episodes = view.input.episodes();
    let rows = EpisodeRows::of(view.sample.episodes());
    let faces = listed
        .items
        .iter()
        .filter_map(|item| {
            let episode = view.sample.episode(episodes.get(*item)?)?;
            Some((
                *item,
                episode.id().clone(),
                view.episode_face(&rows, episode),
            ))
        })
        .collect::<Vec<_>>();
    let thumbnails = Thumbnails::use_camera(controller, None);
    let featured = controller.with_payload(|payload| payload.configured_camera());
    let sample = view.sample.clone();
    let stop = listed.chosen_position();
    rsx! {
        SideReview {
            side,
            sides,
            rows: (!faces.is_empty()).then(|| rsx! {
                ol { class: "curation-entries", "aria-label": "Episodes on this side of the rule",
                    for (position, (item, id, face)) in faces.into_iter().enumerate() {
                        SliceEntry {
                            key: "{id}",
                            face,
                            thumbnails,
                            standing: RowStanding::plain(listed.chosen == Some(item))
                                .roving(TabStop::at(position, stop)),
                            on_select: move |()| tabs.open(&id, None),
                        }
                    }
                }
            }),
            evidence: rsx! {
                EpisodeTabs {
                    open: tabs,
                    render: move |tab: OpenEpisode| {
                        let Some(episode) = sample.episode(tab.episode()).cloned() else {
                            return rsx! {};
                        };
                        rsx! { WholeEpisodeTab { episode, featured_camera: featured.clone(), focus: tab.play() } }
                    },
                }
            },
        }
    }
}

impl RuleView {
    /// An episode's row under a Select: its measures are the facts the
    /// conditions read, each once, then the episode's length unless a
    /// condition reads it; a fact the episode fails a condition on is
    /// marked, so the Left out side says why each row is there.
    fn episode_face(&self, rows: &EpisodeRows, episode: &Episode) -> SliceFace {
        let conditions = match &self.selection {
            Selection::Episodes { conditions } => conditions.as_slice(),
            _ => &[],
        };
        let mut facts = Vec::<EpisodeFact>::new();
        for condition in conditions {
            if !facts.contains(&condition.fact()) {
                facts.push(condition.fact());
            }
        }
        let mut face = SliceFace::episode(rows, episode);
        for fact in facts {
            let Some(reading) = fact.reading(episode) else {
                continue;
            };
            let failing = conditions
                .iter()
                .any(|condition| condition.fact() == fact && !condition.holds(episode));
            face = face.judged(reading, fact.name(), failing);
        }
        face.length_last()
    }
}

/// One stretch of moments on one side of the rule: a run of the input's
/// moments with no moment of the other side between them.
///
/// A Score progress node stores each episode's moments in time order and touching, so
/// a run of consecutive moments of one episode is one stretch of recording.
struct Stretch {
    episode: EpisodeId,
    span: (f64, f64),
    items: Vec<usize>,
    mean: Option<f64>,
}

impl Stretch {
    fn of(rows: &[Moment], items: &[usize]) -> Vec<Self> {
        let mut runs = Vec::<Vec<usize>>::new();
        for item in items.iter().copied() {
            let follows = item
                .checked_sub(1)
                .filter(|previous| rows[*previous].episode() == rows[item].episode());
            match runs.last_mut() {
                Some(run) if follows.is_some() && run.last().copied() == follows => run.push(item),
                _ => runs.push(vec![item]),
            }
        }
        runs.into_iter()
            .map(|items| {
                let (first, last) = (&rows[items[0]], &rows[items[items.len() - 1]]);
                let read = items
                    .iter()
                    .filter_map(|item| rows[*item].progress_rate())
                    .collect::<Vec<_>>();
                Self {
                    episode: first.episode().clone(),
                    span: (first.start_time(), last.end_time()),
                    mean: (!read.is_empty()).then(|| read.iter().sum::<f64>() / read.len() as f64),
                    items,
                }
            })
            .collect()
    }
}

/// One episode's stretches on one side of the rule, and how much of the
/// episode they hold, as its heading says it.
struct StretchGroup {
    episode: EpisodeId,
    /// The stretches' places in the listing.
    positions: Vec<usize>,
    holds: String,
}

impl StretchGroup {
    fn of(stretches: &[Stretch], view: &RuleView, listed_kept: bool) -> Vec<Self> {
        let mut groups = Vec::<Self>::new();
        for (position, stretch) in stretches.iter().enumerate() {
            match groups.last_mut() {
                Some(group) if group.episode == stretch.episode => group.positions.push(position),
                _ => groups.push(Self {
                    episode: stretch.episode.clone(),
                    positions: vec![position],
                    holds: String::new(),
                }),
            }
        }
        for group in &mut groups {
            let seconds = group
                .positions
                .iter()
                .map(|position| stretches[*position].span)
                .map(|(start, end)| (end - start).max(0.0))
                .sum::<f64>();
            let verb = if listed_kept { "keeps" } else { "leaves out" };
            group.holds = match view.sample.duration(&group.episode) {
                Some(duration) => format!(
                    "{verb} {} of {}",
                    seconds.clock_label(),
                    duration.clock_label()
                ),
                None => format!("{verb} {}", seconds.clock_label()),
            };
        }
        groups
    }
}

#[component]
pub(super) fn MomentListing(
    controller: CurationController,
    side: ListedSide,
    tabs: OpenEpisodes,
    sides: Element,
) -> Element {
    let view = side.view.clone();
    let rows = match &view.input {
        Population::Moments(rows) => rows.clone(),
        _ => Vec::new(),
    };
    let chosen = tabs.front().and_then(|tab| view.item_in(&tab));
    let listed = side.listed(chosen);
    let stretches = Stretch::of(&rows, &listed.items);
    // A stretch opens whole, so the first one plays from its start rather
    // than a single moment's.
    let first = side.opening();
    use_effect(use_reactive((&first,), move |(first,)| {
        tabs.open_first(first)
    }));
    let shown = stretches.iter().position(|stretch| {
        listed
            .chosen
            .is_some_and(|item| stretch.items.contains(&item))
    });
    // One heading per episode, as Break down lists its subtasks: the rows
    // under it are its stretches, named by their spans, and the heading says
    // how much of the episode this side holds.
    let groups = StretchGroup::of(&stretches, &view, side.listed_kept);
    let faces = stretches
        .iter()
        .map(|stretch| {
            let duration = view.sample.duration(&stretch.episode).unwrap_or_default();
            let face = SliceFace::stretch(&stretch.episode, stretch.span, duration);
            match stretch.mean {
                Some(mean) => face.measured(
                    ProgressRate::label(mean),
                    "Mean progress rate over this stretch; 1× is the expert rate",
                ),
                None => face,
            }
        })
        .collect::<Vec<_>>();
    // The thumbnails are of the camera the progress was scored from.
    let thumbnails =
        Thumbnails::use_camera(controller, Some(ProgressScorer::WarpSimBottles.camera()));
    let front = tabs.front().map(|tab| tab.episode().clone());
    let cutoff = view.selection.minimum();
    let evidence = view.clone();
    rsx! {
        SideReview {
            side,
            sides,
            rows: (!faces.is_empty()).then(|| rsx! {
                ol { class: "curation-entries curation-entries-grouped", "aria-label": "Stretches of moments on this side of the rule, by episode",
                    for group in groups {
                        li { key: "{group.episode}", class: "curation-entries-group",
                            "aria-current": (front.as_ref() == Some(&group.episode)).then_some("true"),
                            h3 { class: "curation-entries-heading-slot",
                                button {
                                    r#type: "button",
                                    class: "curation-entries-heading",
                                    onclick: {
                                        let episode = group.episode.clone();
                                        move |_| tabs.open(&episode, None)
                                    },
                                    span { class: "curation-entries-heading-name", title: "{group.episode}",
                                        "{Episode::name_of(&group.episode)}" }
                                    span { class: "curation-entries-heading-count", "{group.holds}" }
                                }
                            }
                            ol { class: "curation-entries",
                                for position in group.positions {
                                    SliceEntry {
                                        key: "{stretches[position].span.0}",
                                        face: faces[position].clone(),
                                        thumbnails,
                                        standing: RowStanding::plain(shown == Some(position))
                                            .roving(TabStop::at(position, shown)),
                                        on_select: {
                                            let (episode, span) = (stretches[position].episode.clone(), stretches[position].span);
                                            move |()| tabs.open(&episode, Some(span))
                                        },
                                    }
                                }
                            }
                        }
                    }
                }
            }),
            evidence: rsx! {
                EpisodeTabs {
                    open: tabs,
                    render: move |tab: OpenEpisode| {
                        let Some(episode) = evidence.sample.episode(tab.episode()).cloned() else {
                            return rsx! {};
                        };
                        let moments = rows
                            .iter()
                            .filter(|row| row.episode() == tab.episode())
                            .cloned()
                            .collect::<Vec<_>>();
                        // The stretch's span and its progress rate, as its row says them.
                        let facts = tab
                            .interval()
                            .map(|(start, end)| {
                                let within = moments
                                    .iter()
                                    .filter(|row| row.start_time() >= start && row.end_time() <= end)
                                    .cloned()
                                    .collect::<Vec<_>>();
                                let mut facts = vec![format!("{} – {}", start.clock_label(), end.clock_label())];
                                facts.extend(ProgressRate::mean(&within).map(|rate| format!("mean progress rate {}", ProgressRate::label(rate))));
                                facts
                            })
                            .unwrap_or_default();
                        rsx! {
                            MomentEvidence {
                                episode,
                                moments,
                                facts,
                                cutoff,
                                play: tab.play(),
                            }
                        }
                    },
                }
            },
        }
    }
}
