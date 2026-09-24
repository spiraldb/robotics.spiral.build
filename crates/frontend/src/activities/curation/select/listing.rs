//! One side of a Select's rule, one item at a time beside its recording:
//! whole episodes, subtasks with their stills, or stretches of moments.
//!
//! Every listing names its items by their place in the Select's input, the
//! same number the map and the strip use, so choosing an item anywhere opens
//! its episode in a tab beside the list.
use super::SelectView;
use crate::activities::curation::document::{
    CurationNodeKind, EpisodeFact, MomentRow, Selection, SubtaskRow,
};
use crate::activities::curation::episode_tabs::{EpisodeTabs, OpenEpisode, OpenEpisodes};
use crate::activities::curation::moments::MomentEvidence;
use crate::activities::curation::population::Population;
use crate::activities::curation::poster::{PosterCut, PosterFrames};
use crate::activities::curation::review::{
    EpisodeOrder, EvidenceHeading, SliceEntry, SubtaskContext, SubtaskEntry, SubtaskEvidence,
    SubtaskReview, SubtaskStanding,
};
use crate::activities::curation::state::{CurationController, EpisodeFocus};
use crate::activities::episode::{Episode, EpisodeInspection};
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::EmptyState;

/// The items on the listed side, by their place in the input, and the one
/// playing in the tab in front when it is on this side.
struct ListedItems {
    items: Vec<usize>,
    chosen: Option<usize>,
}

impl ListedItems {
    fn of(kept: &[bool], listed_kept: bool, chosen: Option<usize>) -> Self {
        Self::best_first(kept, listed_kept, chosen, |_| None)
    }
    /// The listed side ordered by `score`, highest first; items without a
    /// score follow in input order.
    fn best_first(
        kept: &[bool],
        listed_kept: bool,
        chosen: Option<usize>,
        score: impl Fn(usize) -> Option<f64>,
    ) -> Self {
        let mut items = kept
            .iter()
            .enumerate()
            .filter(|(_, kept)| **kept == listed_kept)
            .map(|(item, _)| item)
            .collect::<Vec<_>>();
        let rank = |item: usize| score(item).unwrap_or(f64::NEG_INFINITY);
        items.sort_by(|left, right| rank(*right).total_cmp(&rank(*left)));
        let chosen = chosen.filter(|chosen| items.contains(chosen));
        Self { items, chosen }
    }
    /// Where the first listed item plays, which opens by itself so the
    /// recording column is never an empty half beside the list.
    fn first_place(&self, view: &SelectView) -> Option<(EpisodeId, Option<(f64, f64)>)> {
        self.items.first().and_then(|item| view.place(*item))
    }
}

/// What a whole-episode row says about its episode beside its name: its
/// length and its messages, less the one the rule already prints in the
/// score column.
struct EpisodeFacts<'a>(&'a Episode);

impl<'a> EpisodeFacts<'a> {
    fn of(episode: &'a Episode) -> Self {
        Self(episode)
    }
    fn line(&self, judged: Option<EpisodeFact>) -> Vec<String> {
        [EpisodeFact::DurationSeconds, EpisodeFact::Messages]
            .into_iter()
            .filter(|fact| Some(*fact) != judged)
            .filter_map(|fact| {
                let reading = fact.reading(self.0)?;
                Some(match fact {
                    EpisodeFact::Messages => format!("{reading} messages"),
                    _ => reading,
                })
            })
            .collect()
    }
}

/// What an empty side says, rather than an empty column.
#[component]
fn NothingListed(view: SelectView, listed_kept: bool) -> Element {
    rsx! {
        EmptyState { p { "{view.empty_side(listed_kept)}" } }
    }
}

#[component]
pub(super) fn EpisodeListing(
    controller: CurationController,
    view: SelectView,
    kept: Vec<bool>,
    listed_kept: bool,
    tabs: OpenEpisodes,
) -> Element {
    let chosen = tabs.front().and_then(|tab| view.item_in(&tab));
    let listed = ListedItems::of(&kept, listed_kept, chosen);
    let first = listed.first_place(&view);
    use_effect(use_reactive((&first,), move |(first,)| {
        tabs.open_first(first)
    }));
    let Population::Episodes(episodes) = &view.input else {
        return rsx! {};
    };
    if listed.items.is_empty() {
        return rsx! { NothingListed { view: view.clone(), listed_kept } };
    }
    let featured = controller.with_payload(|payload| payload.configured_camera());
    let fact = match &view.selection {
        Selection::Episodes { fact, .. } => Some(*fact),
        _ => None,
    };
    let sample = view.sample.clone();
    rsx! {
        SubtaskReview {
            index: rsx! {
                ol { class: "curation-entries", "aria-label": "Episodes on this side of the rule",
                    for item in listed.items.iter().copied() {
                        {
                            let id = episodes[item].clone();
                            let episode = view.sample.episode(&id);
                            let facts = episode.map(|episode| EpisodeFacts::of(episode).line(fact)).unwrap_or_default();
                            let score = fact.zip(episode).and_then(|(fact, episode)| fact.reading(episode));
                            rsx! {
                                SliceEntry {
                                    key: "{id}",
                                    title: Episode::name_of(&id),
                                    facts,
                                    score,
                                    score_meaning: fact.map(|fact| fact.label().to_owned()),
                                    chosen: listed.chosen == Some(item),
                                    on_select: move |()| tabs.open(&id, None),
                                }
                            }
                        }
                    }
                }
            },
            evidence: rsx! {
                EpisodeTabs {
                    open: tabs,
                    empty: "Choose an episode to watch it.".to_owned(),
                    render: move |tab: OpenEpisode| {
                        let Some(episode) = sample.episode(tab.episode()).cloned() else {
                            return rsx! {};
                        };
                        rsx! {
                            EvidenceHeading {
                                subject: episode.name(),
                                episode: Some(episode.id().clone()),
                                facts: EpisodeFacts::of(&episode).line(None),
                            }
                            div { class: "curation-review-player",
                                EpisodeInspection { episode, featured_camera: featured.clone() }
                            }
                        }
                    },
                }
            },
        }
    }
}

#[component]
pub(super) fn SubtaskListing(
    controller: CurationController,
    view: SelectView,
    kept: Vec<bool>,
    listed_kept: bool,
    tabs: OpenEpisodes,
) -> Element {
    let rows = view.input.subtasks().to_vec();
    let ranking =
        controller.with_payload(|payload| payload.current_ranking(view.measure.id()).cloned());
    let chosen = tabs.front().and_then(|tab| view.item_in(&tab));
    // After a search the closest subtasks come first, as the search lists them.
    let listed = ListedItems::best_first(&kept, listed_kept, chosen, |item| {
        ranking
            .as_ref()
            .and_then(|ranking| ranking.probability(&rows[item]))
    });
    let first = listed.first_place(&view);
    use_effect(use_reactive((&first,), move |(first,)| {
        tabs.open_first(first)
    }));
    let (annotation, camera, dataset) = controller.with_payload(|payload| {
        let annotation = payload.subtask_source(&view.node).map(str::to_owned);
        let camera = annotation.as_deref().and_then(|annotation| {
            match payload.node(annotation).map(|node| node.kind()) {
                Some(CurationNodeKind::BreakDownSubtasks(settings)) => {
                    settings.camera().map(str::to_owned)
                }
                _ => None,
            }
        });
        (annotation, camera, payload.dataset().cloned())
    });
    let context = SubtaskContext::new(
        dataset,
        camera,
        EpisodeOrder::of(rows.iter().map(SubtaskRow::episode)),
    );
    let listed_rows = listed
        .items
        .iter()
        .map(|item| rows[*item].clone())
        .collect::<Vec<_>>();
    let frames = PosterFrames::use_frames(PosterCut::of(&context, &listed_rows));
    if listed.items.is_empty() {
        return rsx! { NothingListed { view: view.clone(), listed_kept } };
    }
    let opening = view.clone();
    rsx! {
        SubtaskReview {
            index: rsx! {
                ol { class: "curation-entries", "aria-label": "Subtasks on this side of the rule",
                    for item in listed.items.iter().copied() {
                        {
                            let row = rows[item].clone();
                            let standing = match ranking.as_ref().and_then(|ranking| ranking.probability(&row)) {
                                Some(probability) => SubtaskStanding::scored(probability, true, listed.chosen == Some(item)),
                                None => SubtaskStanding::plain(listed.chosen == Some(item)),
                            };
                            let opening = opening.clone();
                            rsx! {
                                SubtaskEntry {
                                    key: "{row.episode()}-{row.start_time()}",
                                    episode: Episode::name_of(row.episode()),
                                    row,
                                    standing,
                                    frames,
                                    on_select: move |()| opening.open(tabs, item),
                                }
                            }
                        }
                    }
                }
            },
            evidence: rsx! {
                EpisodeTabs {
                    open: tabs,
                    empty: "Choose a subtask to watch it play.".to_owned(),
                    render: move |tab: OpenEpisode| {
                        let Some(row) = rows.iter().find(|row| tab.holds(row.episode(), row.start_time())).cloned() else {
                            return rsx! {};
                        };
                        rsx! {
                            SubtaskEvidence {
                                play: tab.play(),
                                episode_rows: rows
                                    .iter()
                                    .filter(|other| other.episode() == row.episode())
                                    .cloned()
                                    .collect::<Vec<_>>(),
                                context: context.clone(),
                                on_open: {
                                    let focus = annotation.clone().map(|annotation| {
                                        EpisodeFocus::new(
                                            annotation,
                                            row.episode().clone(),
                                            Some((row.start_time(), row.end_time())),
                                        )
                                    });
                                    move |()| {
                                        if let Some(focus) = focus.clone() {
                                            controller.focus_episode(focus);
                                        }
                                    }
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

/// One stretch of moments on one side of the rule: a run of the input's
/// moments with no moment of the other side between them.
///
/// A detection stores each episode's moments in time order and touching, so
/// a run of consecutive moments of one episode is one stretch of recording.
struct Stretch {
    episode: EpisodeId,
    span: (f64, f64),
    items: Vec<usize>,
    mean: Option<f64>,
}

impl Stretch {
    fn of(rows: &[MomentRow], items: &[usize]) -> Vec<Self> {
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
                    .filter_map(|item| rows[*item].velocity())
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

#[component]
pub(super) fn MomentListing(
    view: SelectView,
    kept: Vec<bool>,
    listed_kept: bool,
    tabs: OpenEpisodes,
) -> Element {
    let rows = match &view.input {
        Population::Moments(rows) => rows.clone(),
        _ => Vec::new(),
    };
    let chosen = tabs.front().and_then(|tab| view.item_in(&tab));
    let listed = ListedItems::of(&kept, listed_kept, chosen);
    let stretches = Stretch::of(&rows, &listed.items);
    // A stretch opens whole, so the first one plays from its first second to
    // its last rather than for one second.
    let first = stretches
        .first()
        .map(|stretch| (stretch.episode.clone(), Some(stretch.span)));
    use_effect(use_reactive((&first,), move |(first,)| {
        tabs.open_first(first)
    }));
    if listed.items.is_empty() {
        return rsx! { NothingListed { view: view.clone(), listed_kept } };
    }
    let shown = stretches.iter().position(|stretch| {
        listed
            .chosen
            .is_some_and(|item| stretch.items.contains(&item))
    });
    let cutoff = view.selection.threshold();
    let evidence = view.clone();
    rsx! {
        SubtaskReview {
            index: rsx! {
                ol { class: "curation-entries", "aria-label": "Stretches of moments on this side of the rule",
                    for (position , stretch) in stretches.iter().enumerate() {
                        {
                            let duration = view.sample.duration(&stretch.episode).unwrap_or_default().max(f64::EPSILON);
                            let episode = stretch.episode.clone();
                            let (start, end) = stretch.span;
                            rsx! {
                                SliceEntry {
                                    key: "{stretch.episode}-{start}",
                                    title: Episode::name_of(&stretch.episode),
                                    facts: vec![format!("{} – {}", start.clock_label(), end.clock_label())],
                                    place: Some((start / duration, end / duration)),
                                    score: stretch.mean.map(|mean| format!("{mean:.2}")),
                                    score_meaning: "Mean progress per second over this stretch".to_owned(),
                                    chosen: shown == Some(position),
                                    on_select: move |()| tabs.open(&episode, Some((start, end))),
                                }
                            }
                        }
                    }
                }
            },
            evidence: rsx! {
                EpisodeTabs {
                    open: tabs,
                    empty: "Choose a stretch to watch it.".to_owned(),
                    render: move |tab: OpenEpisode| {
                        let Some(episode) = evidence.sample.episode(tab.episode()).cloned() else {
                            return rsx! {};
                        };
                        let moments = rows
                            .iter()
                            .filter(|row| row.episode() == tab.episode())
                            .cloned()
                            .collect::<Vec<_>>();
                        let facts = tab
                            .interval()
                            .map(|(start, end)| vec![format!("{} – {}", start.clock_label(), end.clock_label())])
                            .unwrap_or_default();
                        rsx! {
                            MomentEvidence {
                                curve: evidence.curve(tab.episode()),
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
