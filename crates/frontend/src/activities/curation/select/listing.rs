//! One side of a Select, one row at a time beside its recording: whole
//! episodes when it keeps or drops episodes whole, else the kept or left-out
//! stretches of each episode under its heading.
use super::SelectView;
use super::evidence::SelectEvidence;
use crate::activities::curation::document::{EpisodeNumber, SelectCondition};
use crate::activities::curation::episode_tabs::{EpisodeTabs, OpenEpisode, OpenEpisodes};
use crate::activities::curation::population::KeptRanges;
use crate::activities::curation::review::{
    IndexBar, RowStanding, SliceEntry, SliceFace, SubtaskReview, TabStop,
};
use crate::activities::curation::select::reading::ConditionReading;
use crate::activities::curation::thumbnail::Thumbnails;
use crate::activities::episode::{Episode, EpisodeRows};
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::EpisodeId;
use spiral_robotics_ui::{ActionPurpose, ModeButton, ModeGroup};

/// Which side of the Select the index lists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Showing {
    Kept,
    LeftOut,
}

/// One episode on one side: the ranges of it on that side, in time order.
#[derive(Clone, Debug, PartialEq)]
struct ListedEpisode {
    episode: EpisodeId,
    ranges: KeptRanges,
}

/// What one side of the Select lists.
#[derive(Clone, Debug, PartialEq)]
struct ListedSide(Vec<ListedEpisode>);

impl ListedSide {
    /// The kept side: every episode the Select keeps, with what it keeps;
    /// or the left-out side: what reaches it that it does not keep.
    fn of(view: &SelectView, showing: Showing) -> Self {
        Self(
            view.reading
                .input()
                .episodes()
                .into_iter()
                .filter_map(|episode| {
                    let kept = view.kept(&episode);
                    let ranges = match showing {
                        Showing::Kept => kept,
                        Showing::LeftOut if view.reading.output().of(&episode).is_none() => {
                            view.reached(&episode)
                        }
                        Showing::LeftOut => view.reached(&episode).without(&kept),
                    };
                    (!ranges.is_empty()).then_some(ListedEpisode { episode, ranges })
                })
                .collect(),
        )
    }
    /// How many rows it lists: episodes, or the stretches of them.
    fn rows(&self, narrows: bool) -> usize {
        if narrows {
            self.0
                .iter()
                .map(|listed| listed.ranges.ranges().len())
                .sum()
        } else {
            self.0.len()
        }
    }
    /// Where its first row plays: an episode whole, or its first stretch.
    fn opening(&self, narrows: bool) -> Option<(EpisodeId, Option<(f64, f64)>)> {
        let first = self.0.first()?;
        Some((
            first.episode.clone(),
            narrows
                .then(|| first.ranges.ranges().first().copied())
                .flatten(),
        ))
    }
}

/// One side of the Select listed beside the recording: the Kept and Left
/// out segments over the index, then the rows of the side shown.
#[component]
pub(super) fn SelectListing(view: SelectView, tabs: OpenEpisodes) -> Element {
    let mut showing = use_signal(|| Showing::Kept);
    let narrows = view.narrows();
    let (kept_side, left_side) = (
        ListedSide::of(&view, Showing::Kept),
        ListedSide::of(&view, Showing::LeftOut),
    );
    let side = match showing() {
        Showing::Kept => kept_side.clone(),
        Showing::LeftOut => left_side.clone(),
    };
    // The side's first row plays by itself, so the recording column is never
    // an empty half beside the list.
    let first = side.opening(narrows);
    use_effect(use_reactive((&first,), move |(first,)| {
        tabs.open_first(first)
    }));
    let switch = move |to: Showing, listed: &ListedSide| {
        let opening = listed.opening(narrows);
        move |_| {
            if showing() != to {
                showing.set(to);
                if let Some((episode, span)) = opening.clone() {
                    tabs.open(&episode, span);
                }
            }
        }
    };
    let sides = rsx! {
        ModeGroup { label: "Which side of the Select to list".to_owned(), class: "curation-select-sides".to_owned(),
            ModeButton {
                purpose: ActionPurpose::Select,
                checked: showing() == Showing::Kept,
                onclick: switch(Showing::Kept, &kept_side),
                "Kept {kept_side.rows(narrows)}"
            }
            ModeButton {
                purpose: ActionPurpose::Select,
                checked: showing() == Showing::LeftOut,
                onclick: switch(Showing::LeftOut, &left_side),
                "Left out {left_side.rows(narrows)}"
            }
        }
    };
    let empty = match showing() {
        _ if view.reading.input().len() == 0 => "Nothing reaches this Select yet.",
        Showing::Kept => "Nothing passes this Select.",
        Showing::LeftOut => "Nothing is left out.",
    };
    let evidence = view.clone();
    rsx! {
        SubtaskReview {
            index: rsx! {
                IndexBar { {sides} }
                if side.0.is_empty() {
                    p { class: "curation-review-note", role: "status", "{empty}" }
                } else if narrows {
                    StretchRows { view: view.clone(), side, showing: showing(), tabs }
                } else {
                    EpisodeRowsOfSide { view: view.clone(), side, tabs }
                }
            },
            evidence: rsx! {
                EpisodeTabs {
                    open: tabs,
                    render: move |tab: OpenEpisode| {
                        let Some(episode) = evidence.sample.episode(tab.episode()).cloned() else {
                            return rsx! {};
                        };
                        rsx! { SelectEvidence { view: evidence.clone(), episode, play: tab.play() } }
                    },
                }
            },
        }
    }
}

/// Whole episodes, each with the numbers the Select's conditions read, a
/// number an episode fails on marked, so the Left out side says why each row
/// is there.
#[component]
fn EpisodeRowsOfSide(view: SelectView, side: ListedSide, tabs: OpenEpisodes) -> Element {
    let thumbnails = Thumbnails::use_camera(view.controller, None);
    let rows = EpisodeRows::of(view.sample.episodes());
    let front = tabs.front();
    let chosen = side.0.iter().position(|listed| {
        front
            .as_ref()
            .is_some_and(|tab| tab.episode() == &listed.episode && tab.interval().is_none())
    });
    let faces = side
        .0
        .iter()
        .filter_map(|listed| {
            let episode = view.sample.episode(&listed.episode)?;
            Some((listed.episode.clone(), view.episode_face(&rows, episode)))
        })
        .collect::<Vec<_>>();
    rsx! {
        ol { class: "curation-entries", "aria-label": "Episodes on this side of the Select",
            for (position, (id, face)) in faces.into_iter().enumerate() {
                SliceEntry {
                    key: "{id}",
                    face,
                    thumbnails,
                    standing: RowStanding::plain(chosen == Some(position))
                        .roving(TabStop::at(position, chosen)),
                    on_select: move |()| tabs.open(&id, None),
                }
            }
        }
    }
}

/// The stretches on one side, under one heading per episode saying how much
/// of it the side holds; each stretch plays to its end.
#[component]
fn StretchRows(
    view: SelectView,
    side: ListedSide,
    showing: Showing,
    tabs: OpenEpisodes,
) -> Element {
    let thumbnails = Thumbnails::use_camera(view.controller, None);
    let front = tabs.front();
    let verb = match showing {
        Showing::Kept => "keeps",
        Showing::LeftOut => "leaves out",
    };
    let stretches = side
        .0
        .iter()
        .flat_map(|listed| {
            listed
                .ranges
                .ranges()
                .iter()
                .map(move |span| (listed.episode.clone(), *span))
        })
        .collect::<Vec<_>>();
    let shown = stretches.iter().position(|(episode, (start, _))| {
        front.as_ref().is_some_and(|tab| tab.holds(episode, *start))
    });
    let mut position = 0;
    let groups = side
        .0
        .iter()
        .map(|listed| {
            let duration = view.sample.duration(&listed.episode);
            let seconds = listed.ranges.seconds().clock_label();
            let holds = match duration {
                Some(duration) => format!("{verb} {seconds} of {}", duration.clock_label()),
                None => format!("{verb} {seconds}"),
            };
            let rows = listed
                .ranges
                .ranges()
                .iter()
                .map(|span| {
                    let face =
                        view.stretch_face(&listed.episode, *span, duration.unwrap_or_default());
                    position += 1;
                    (position - 1, *span, face)
                })
                .collect::<Vec<_>>();
            (listed.episode.clone(), holds, rows)
        })
        .collect::<Vec<_>>();
    rsx! {
        ol { class: "curation-entries curation-entries-grouped", "aria-label": "Stretches on this side of the Select, by episode",
            for (episode, holds, rows) in groups {
                li { key: "{episode}", class: "curation-entries-group",
                    "aria-current": front.as_ref().is_some_and(|tab| tab.episode() == &episode).then_some("true"),
                    h3 { class: "curation-entries-heading-slot",
                        button {
                            r#type: "button",
                            class: "curation-entries-heading",
                            onclick: {
                                let episode = episode.clone();
                                move |_| tabs.open(&episode, None)
                            },
                            span { class: "curation-entries-heading-name", title: "{episode}",
                                "{Episode::name_of(&episode)}" }
                            span { class: "curation-entries-heading-count", "{holds}" }
                        }
                    }
                    ol { class: "curation-entries",
                        for (position, span, face) in rows {
                            SliceEntry {
                                key: "{span.0}",
                                face,
                                thumbnails,
                                standing: RowStanding::plain(shown == Some(position))
                                    .roving(TabStop::at(position, shown)),
                                on_select: {
                                    let episode = episode.clone();
                                    move |()| tabs.open(&episode, Some(span))
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

impl SelectView {
    /// An episode's row: the numbers its episode conditions read, each
    /// once, then its length; a number the episode fails on is marked.
    fn episode_face(&self, rows: &EpisodeRows, episode: &Episode) -> SliceFace {
        let place = self
            .reading
            .input()
            .episodes()
            .iter()
            .position(|id| id == episode.id());
        let mut face = SliceFace::episode(rows, episode);
        let mut shown = Vec::<EpisodeNumber>::new();
        for (index, condition) in self.selection.conditions().iter().enumerate() {
            let SelectCondition::Episodes(condition) = condition else {
                continue;
            };
            if shown.contains(condition.number()) {
                continue;
            }
            shown.push(condition.number().clone());
            let (number, failing) = match (self.reading.condition(index), place) {
                (Some(ConditionReading::Episodes { numbers, holds }), Some(place)) => {
                    (numbers[place], !holds[place])
                }
                _ => (None, true),
            };
            let reading = match (condition.number(), number) {
                (EpisodeNumber::Fact(fact), Some(_)) => fact.reading(episode),
                (EpisodeNumber::MostAtOnce(_), Some(number)) => Some(format!("{number:.0}")),
                (EpisodeNumber::DurationPer(_), Some(number)) => Some(format!("{number:.1} s")),
                (_, None) => Some("—".to_owned()),
            };
            let name = self.with_graph(|graph| condition.number().name(graph));
            if let Some(reading) = reading {
                face = face.judged(reading, &name, failing);
            }
        }
        face.length_last()
    }

    /// A stretch's row: its span within its episode, and its mean progress
    /// rate when the Select keeps by one.
    fn stretch_face(&self, episode: &EpisodeId, span: (f64, f64), duration: f64) -> SliceFace {
        let face = SliceFace::stretch(episode, span, duration);
        let progress = self
            .selection
            .conditions()
            .iter()
            .find_map(|condition| match condition {
                SelectCondition::Progressing { node, .. } => Some(node.clone()),
                _ => None,
            });
        let mean = progress
            .and_then(|node| self.controller.timeline(&node, episode))
            .and_then(|timeline| timeline.mean_within(span));
        match mean {
            Some(mean) => face.measured(
                crate::activities::curation::score_progress::ProgressRate::label(mean),
                "Mean progress rate over this stretch; 1× is the expert rate",
            ),
            None => face,
        }
    }
}
