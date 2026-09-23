//! One side of a Select's rule, one item at a time beside its recording:
//! whole episodes, subtasks with their stills, or stretches of moments.
//!
//! Every listing names its items by their place in the Select's input, the
//! same number the map and the strip use, so choosing an item anywhere opens
//! it here.
use super::SelectView;
use crate::activities::curation::document::{CurationNodeKind, MomentRow, Selection, SubtaskRow};
use crate::activities::curation::moments::MomentEvidence;
use crate::activities::curation::population::Population;
use crate::activities::curation::poster::{PosterCut, PosterFrames};
use crate::activities::curation::review::{
    EpisodeOrder, NoSubtaskChosen, SliceEntry, SubtaskContext, SubtaskEntry, SubtaskEvidence,
    SubtaskReview, SubtaskStanding,
};
use crate::activities::curation::state::{CurationController, EpisodeFocus};
use crate::activities::episode::{Episode, EpisodeInspection};
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::EmptyState;

/// The items on the listed side, by their place in the input; the chosen one
/// when it is on this side, else the first listed.
struct ListedItems {
    items: Vec<usize>,
    shown: Option<usize>,
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
        let shown = chosen
            .filter(|chosen| items.contains(chosen))
            .or_else(|| items.first().copied());
        Self { items, shown }
    }
}

/// What an empty side says, rather than an empty column.
#[component]
fn NothingListed(listed_kept: bool) -> Element {
    rsx! {
        EmptyState {
            p {
                if listed_kept { "The rule keeps nothing that reaches it." } else { "The rule keeps everything that reaches it." }
            }
        }
    }
}

#[component]
pub(super) fn EpisodeListing(
    controller: CurationController,
    view: SelectView,
    kept: Vec<bool>,
    listed_kept: bool,
    chosen: Signal<Option<usize>>,
) -> Element {
    let Population::Episodes(episodes) = &view.input else {
        return rsx! {};
    };
    let listed = ListedItems::of(&kept, listed_kept, chosen());
    if listed.items.is_empty() {
        return rsx! { NothingListed { listed_kept } };
    }
    let featured = controller.with_payload(|payload| payload.configured_camera());
    let fact = match &view.selection {
        Selection::Episodes { fact, .. } => Some(*fact),
        _ => None,
    };
    let mut chosen = chosen;
    rsx! {
        SubtaskReview {
            index: rsx! {
                ol { class: "curation-entries", "aria-label": "Episodes on this side of the rule",
                    for item in listed.items.iter().copied() {
                        {
                            let id = episodes[item].clone();
                            let episode = view.sample.episode(&id);
                            let facts = episode.map(|episode| {
                                vec![
                                    episode.facts().duration_seconds().unwrap_or_default().clock_label(),
                                    format!("{} messages", episode.facts().message_count().unwrap_or_default()),
                                ]
                            }).unwrap_or_default();
                            let score = fact
                                .zip(episode)
                                .and_then(|(fact, episode)| fact.of(episode))
                                .map(|value| format!("{value:.1}"));
                            rsx! {
                                SliceEntry {
                                    key: "{id}",
                                    title: Episode::name_of(&id),
                                    facts,
                                    place: (0.0, 1.0),
                                    score,
                                    chosen: listed.shown == Some(item),
                                    on_select: move |()| chosen.set(Some(item)),
                                }
                            }
                        }
                    }
                }
            },
            evidence: match listed.shown.and_then(|item| view.sample.episode(&episodes[item])).cloned() {
                Some(episode) => rsx! {
                    div { class: "curation-review-player",
                        EpisodeInspection { key: "{episode.id()}", episode, featured_camera: featured }
                    }
                },
                None => rsx! { NoSubtaskChosen { prompt: "Choose an episode to watch it." } },
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
    chosen: Signal<Option<usize>>,
) -> Element {
    let rows = view.input.subtasks().to_vec();
    let ranking =
        controller.with_payload(|payload| payload.current_ranking(view.measure.id()).cloned());
    // After a search the closest subtasks come first, as the search lists them.
    let listed = ListedItems::best_first(&kept, listed_kept, chosen(), |item| {
        ranking
            .as_ref()
            .and_then(|ranking| ranking.probability(&rows[item]))
    });
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
        return rsx! { NothingListed { listed_kept } };
    }
    let mut chosen = chosen;
    rsx! {
        SubtaskReview {
            index: rsx! {
                ol { class: "curation-entries", "aria-label": "Subtasks on this side of the rule",
                    for item in listed.items.iter().copied() {
                        {
                            let row = rows[item].clone();
                            let standing = match ranking.as_ref().and_then(|ranking| ranking.probability(&row)) {
                                Some(probability) => SubtaskStanding::scored(probability, true, listed.shown == Some(item)),
                                None => SubtaskStanding::plain(listed.shown == Some(item)),
                            };
                            rsx! {
                                SubtaskEntry {
                                    key: "{row.episode()}-{row.start_time()}",
                                    episode: Episode::name_of(row.episode()),
                                    row,
                                    standing,
                                    frames,
                                    on_select: move |()| chosen.set(Some(item)),
                                }
                            }
                        }
                    }
                }
            },
            evidence: match listed.shown.map(|item| rows[item].clone()) {
                Some(row) => rsx! {
                    SubtaskEvidence {
                        key: "{row.episode()}-{row.start_time()}",
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
                },
                None => rsx! { NoSubtaskChosen { prompt: "Choose a subtask to watch it play." } },
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
    chosen: Signal<Option<usize>>,
) -> Element {
    let Population::Moments(rows) = &view.input else {
        return rsx! {};
    };
    let listed = ListedItems::of(&kept, listed_kept, chosen());
    if listed.items.is_empty() {
        return rsx! { NothingListed { listed_kept } };
    }
    let stretches = Stretch::of(rows, &listed.items);
    let shown = stretches
        .iter()
        .position(|stretch| {
            listed
                .shown
                .is_some_and(|item| stretch.items.contains(&item))
        })
        .unwrap_or_default();
    let cutoff = view.selection.threshold();
    let mut chosen = chosen;
    rsx! {
        SubtaskReview {
            index: rsx! {
                ol { class: "curation-entries", "aria-label": "Stretches of moments on this side of the rule",
                    for (position , stretch) in stretches.iter().enumerate() {
                        {
                            let duration = view.sample.duration(&stretch.episode).unwrap_or_default().max(f64::EPSILON);
                            let first = stretch.items[0];
                            let (start, end) = stretch.span;
                            rsx! {
                                SliceEntry {
                                    key: "{stretch.episode}-{start}",
                                    title: Episode::name_of(&stretch.episode),
                                    facts: vec![
                                        format!("{} – {}", start.clock_label(), end.clock_label()),
                                        (end - start).clock_label(),
                                    ],
                                    place: (start / duration, end / duration),
                                    score: stretch.mean.map(|mean| format!("{mean:.2}")),
                                    chosen: position == shown,
                                    on_select: move |()| chosen.set(Some(first)),
                                }
                            }
                        }
                    }
                }
            },
            evidence: match stretches.get(shown) {
                Some(stretch) => {
                    let episode = view.sample.episode(&stretch.episode).cloned();
                    let moments = rows
                        .iter()
                        .filter(|row| row.episode() == &stretch.episode)
                        .cloned()
                        .collect::<Vec<_>>();
                    match episode {
                        Some(episode) => rsx! {
                            MomentEvidence {
                                key: "{episode.id()}",
                                episode,
                                moments,
                                cutoff,
                                focus: Some(stretch.span),
                            }
                        },
                        None => rsx! {},
                    }
                }
                None => rsx! {},
            },
        }
    }
}
