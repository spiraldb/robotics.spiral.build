//! The episode map: every measured episode as one thin lane, every subtask row
//! as a segment of its lane, placed by its interval.
//!
//! Lanes span the full width whatever their episode's length, so a segment's
//! position says when in its episode the row happens and episodes of different
//! lengths line up by phase. Search paints segments by how likely each is the
//! same operation; grouping paints them by category. Either way the map is the
//! dataset at a glance — the index beside it is the same rows one at a time.
use super::coverage::MeasuredEpisodes;
use crate::activities::episode::Episode;
use crate::activities::workflow::explore::document::SubtaskRow;
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeId};
use std::collections::BTreeMap;

/// The most episodes one annotation reads, and so the most the map lays out.
const MAX_SPANNED_EPISODES: usize = 2_000;
/// A reveal cascades across the map in about this long, however many lanes.
const REVEAL_SPREAD_MS: f64 = 700.0;

/// How long each episode of the dataset is, so a row's interval can be drawn
/// as a share of its own episode.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct EpisodeSpans(BTreeMap<EpisodeId, f64>);

impl EpisodeSpans {
    /// The dataset's episode lengths, once one listing query has answered.
    pub(super) fn use_spans(dataset: Option<DatasetSelection>) -> Option<Self> {
        let spans = use_resource(use_reactive(&dataset, |dataset| async move {
            let dataset = dataset?;
            let page = Episode::page(&dataset, 0, MAX_SPANNED_EPISODES)
                .await
                .ok()?;
            Some(Self(
                page.episodes()
                    .iter()
                    .filter_map(|episode| {
                        Some((episode.id().clone(), episode.facts().duration_seconds()?))
                    })
                    .filter(|(_, seconds)| *seconds > 0.0)
                    .collect(),
            ))
        }));
        spans.read().clone().flatten()
    }
    pub(super) fn duration(&self, episode: &EpisodeId) -> Option<f64> {
        self.0.get(episode).copied()
    }
    #[cfg(test)]
    pub(super) fn fixture(spans: &[(EpisodeId, f64)]) -> Self {
        Self(spans.iter().cloned().collect())
    }
}

/// One row's place in its lane, as fractions of the episode, and what the
/// row is in words.
#[derive(Clone, Debug, PartialEq)]
struct LaneSegment {
    row: usize,
    left: f64,
    width: f64,
    label: String,
}

#[derive(Clone, Debug, PartialEq)]
struct EpisodeLane {
    episode: EpisodeId,
    segments: Vec<LaneSegment>,
}

/// Every measured episode's lane, in the order the annotation measured them.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct EpisodeLanes(Vec<EpisodeLane>);

impl EpisodeLanes {
    /// Lay out `rows` against their episodes' lengths. An episode whose length
    /// is unknown has no lane rather than a guessed one, and a row of an
    /// episode the annotation never measured has no segment.
    pub(super) fn of(
        measured: &MeasuredEpisodes,
        rows: &[SubtaskRow],
        spans: &EpisodeSpans,
    ) -> Self {
        Self(
            measured
                .iter()
                .filter_map(|episode| {
                    let duration = spans.duration(episode)?;
                    let segments = rows
                        .iter()
                        .enumerate()
                        .filter(|(_, row)| row.episode() == episode)
                        .map(|(index, row)| {
                            let left = (row.start_time() / duration).clamp(0.0, 1.0);
                            let right = (row.end_time() / duration).clamp(left, 1.0);
                            LaneSegment {
                                row: index,
                                left,
                                width: right - left,
                                label: format!(
                                    "{} · {} · {} – {}",
                                    row.description(),
                                    Episode::name_of(episode),
                                    row.start_time().clock_label(),
                                    row.end_time().clock_label()
                                ),
                            }
                        })
                        .collect();
                    Some(EpisodeLane {
                        episode: episode.clone(),
                        segments,
                    })
                })
                .collect(),
        )
    }
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Where a row sits: its lane and its segment's middle, for a reveal to
    /// spread from.
    fn locate(&self, row: usize) -> Option<(usize, f64)> {
        self.0.iter().enumerate().find_map(|(lane, episode)| {
            episode
                .segments
                .iter()
                .find(|segment| segment.row == row)
                .map(|segment| (lane, segment.left + segment.width / 2.0))
        })
    }
}

/// How one row's segment is painted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum SegmentPaint {
    /// Nothing has been asked about this row yet.
    Unasked,
    /// How likely a search thinks this row is the same operation.
    Match(f64),
    /// The category the row joined, as its hue; whether jev hesitated over
    /// it; and whether another category is open, which fades this one.
    Category {
        hue: usize,
        hesitant: bool,
        faded: bool,
    },
}

impl SegmentPaint {
    fn colour(self) -> String {
        match self {
            Self::Unasked => "--paint: var(--explore-map-unasked)".to_owned(),
            Self::Match(probability) => format!(
                "--paint: color-mix(in srgb, var(--explore-map-match) {:.0}%, var(--explore-map-unasked))",
                probability.clamp(0.0, 1.0) * 100.0
            ),
            Self::Category { hue, .. } => format!("--paint: var(--tone-category-{})", hue + 1),
        }
    }
    fn hesitant(self) -> bool {
        matches!(self, Self::Category { hesitant: true, .. })
    }
    fn faded(self) -> bool {
        matches!(self, Self::Category { faded: true, .. })
    }
    fn detail(self) -> Option<String> {
        match self {
            Self::Match(probability) => Some(format!("{:.0}% same operation", probability * 100.0)),
            Self::Unasked | Self::Category { .. } => None,
        }
    }
}

/// How the map shows a new answer arriving.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum MapReveal {
    /// Nothing new: paint in place.
    Still,
    /// A model is reading the rows: a soft light sweeps the lanes, and the
    /// row asked about, if any, pulses.
    Reading,
    /// A search answered: segments light up spreading out from the row the
    /// query came from, or from the first lane when it was typed.
    Ripple { origin: Option<usize> },
    /// A grouping landed: lanes take their colours top to bottom, in step
    /// with the tokens leaving them for their categories.
    Pour,
}

/// Everything the map paints, apart from where the segments are.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct MapPaint {
    /// One paint per row of the table, in table order.
    segments: Vec<SegmentPaint>,
    /// The row the evidence column is showing.
    chosen: Option<usize>,
    /// The row a search asked about.
    source: Option<usize>,
    reveal: MapReveal,
    /// Bumped with every answer, so the reveal plays again for the next one.
    generation: u32,
}

impl MapPaint {
    pub(super) fn new(segments: Vec<SegmentPaint>, chosen: Option<usize>) -> Self {
        Self {
            segments,
            chosen,
            source: None,
            reveal: MapReveal::Still,
            generation: 0,
        }
    }
    pub(super) fn asked_about(mut self, source: Option<usize>) -> Self {
        self.source = source;
        self
    }
    pub(super) fn revealing(mut self, reveal: MapReveal, generation: u32) -> Self {
        self.reveal = reveal;
        self.generation = generation;
        self
    }
    fn paint(&self, row: usize) -> SegmentPaint {
        self.segments
            .get(row)
            .copied()
            .unwrap_or(SegmentPaint::Unasked)
    }
    fn reveal_key(&self) -> &'static str {
        match self.reveal {
            MapReveal::Still => "still",
            MapReveal::Reading => "reading",
            MapReveal::Ripple { .. } => "ripple",
            MapReveal::Pour => "pour",
        }
    }
    /// How long a segment waits before its reveal plays.
    fn delay(&self, lanes: &EpisodeLanes, lane: usize, segment: &LaneSegment) -> f64 {
        let spacing = REVEAL_SPREAD_MS / lanes.0.len().max(1) as f64;
        match self.reveal {
            MapReveal::Still | MapReveal::Reading => 0.0,
            MapReveal::Pour => lane as f64 * spacing,
            MapReveal::Ripple { origin } => {
                let Some((from_lane, from_x)) = origin.and_then(|row| lanes.locate(row)) else {
                    return lane as f64 * spacing;
                };
                let across = (segment.left + segment.width / 2.0 - from_x).abs() * 6.0;
                ((lane.abs_diff(from_lane) as f64 + across) * spacing).min(REVEAL_SPREAD_MS)
            }
        }
    }
}

/// The map, with the count it illustrates above it.
///
/// A segment is a pointer shortcut to its row; keyboard users reach the same
/// row in the index beside the map, so no segment is a tab stop.
#[component]
pub(super) fn EpisodeMap(
    lanes: EpisodeLanes,
    paint: MapPaint,
    /// "Found in 23 of 39 episodes (best match ≥ 50%)" — the map's meaning in
    /// words, because colour is never the only carrier.
    summary: String,
    on_select: EventHandler<usize>,
) -> Element {
    rsx! {
        div { class: "explore-map",
            p { class: "explore-map-summary", "{summary}" }
            if !lanes.is_empty() {
                div {
                    key: "{paint.generation}",
                    class: "explore-map-lanes",
                    role: "group",
                    "aria-label": "Every measured episode, each subtask placed where it happens",
                    "data-reveal": paint.reveal_key(),
                    for (lane , episode) in lanes.0.iter().enumerate() {
                        div {
                            key: "{episode.episode}",
                            class: "explore-map-lane",
                            for segment in episode.segments.iter() {
                                {
                                    let row = segment.row;
                                    let look = paint.paint(row);
                                    let label = match look.detail() {
                                        Some(detail) => format!("{} · {detail}", segment.label),
                                        None => segment.label.clone(),
                                    };
                                    let delay = paint.delay(&lanes, lane, segment);
                                    let style = format!(
                                        "left: {:.3}%; width: {:.3}%; --delay: {delay:.0}ms; {}",
                                        segment.left * 100.0,
                                        segment.width * 100.0,
                                        look.colour()
                                    );
                                    rsx! {
                                        button {
                                            key: "{row}",
                                            r#type: "button",
                                            class: "explore-map-segment",
                                            tabindex: "-1",
                                            style: "{style}",
                                            title: "{label}",
                                            "aria-label": "{label}",
                                            "aria-current": (paint.chosen == Some(row)).then_some("true"),
                                            "data-row": "{row}",
                                            // A token leaving this segment for its
                                            // category waits the same beat.
                                            "data-delay": "{delay:.0}",
                                            "data-source": (paint.source == Some(row)).then_some("true"),
                                            "data-hesitant": look.hesitant().then_some("true"),
                                            "data-faded": look.faded().then_some("true"),
                                            onclick: move |_| on_select.call(row),
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(episode: u32, start: f64, end: f64) -> SubtaskRow {
        SubtaskRow::new(
            "annotate-1",
            EpisodeId::fixture(episode),
            start,
            end,
            "fold",
        )
        .unwrap()
    }

    #[test]
    fn a_row_is_placed_as_a_share_of_its_own_episode() {
        let rows = vec![row(1, 5.0, 10.0), row(2, 5.0, 10.0), row(1, 15.0, 25.0)];
        let lanes = EpisodeLanes::of(
            &MeasuredEpisodes::of(&[EpisodeId::fixture(1), EpisodeId::fixture(2)]),
            &rows,
            &EpisodeSpans::fixture(&[(EpisodeId::fixture(1), 20.0), (EpisodeId::fixture(2), 40.0)]),
        );
        let placed = |lane: usize| {
            lanes.0[lane]
                .segments
                .iter()
                .map(|segment| (segment.row, segment.left, segment.width))
                .collect::<Vec<_>>()
        };
        assert_eq!(lanes.0.len(), 2);
        assert_eq!(
            placed(0),
            [(0, 0.25, 0.25), (2, 0.75, 0.25)],
            "an interval past the recorded end is drawn up to it"
        );
        assert_eq!(placed(1), [(1, 0.125, 0.125)]);
        assert_eq!(
            lanes.0[0].segments[0].label,
            "fold · Episode …00000001 · 0:05.0 – 0:10.0"
        );
        assert_eq!(lanes.locate(2), Some((0, 0.875)));
    }

    #[test]
    fn an_episode_of_unknown_length_or_never_measured_has_no_lane() {
        let rows = vec![row(1, 0.0, 1.0), row(3, 0.0, 1.0)];
        let lanes = EpisodeLanes::of(
            &MeasuredEpisodes::of(&[EpisodeId::fixture(1), EpisodeId::fixture(2)]),
            &rows,
            &EpisodeSpans::fixture(&[(EpisodeId::fixture(1), 10.0), (EpisodeId::fixture(3), 10.0)]),
        );
        assert_eq!(lanes.0.len(), 1);
        assert_eq!(lanes.0[0].episode, EpisodeId::fixture(1));
        assert_eq!(lanes.locate(1), None);
    }

    #[test]
    fn a_ripple_spreads_from_the_row_asked_about_and_a_pour_runs_down_the_lanes() {
        let rows = vec![row(1, 0.0, 2.0), row(2, 0.0, 2.0), row(3, 0.0, 2.0)];
        let episodes = [1, 2, 3].map(EpisodeId::fixture);
        let lanes = EpisodeLanes::of(
            &MeasuredEpisodes::of(&episodes),
            &rows,
            &EpisodeSpans::fixture(&episodes.clone().map(|episode| (episode, 10.0))),
        );
        let segment = |lane: usize| lanes.0[lane].segments[0].clone();
        let ripple =
            MapPaint::new(Vec::new(), None).revealing(MapReveal::Ripple { origin: Some(1) }, 1);
        assert_eq!(ripple.delay(&lanes, 1, &segment(1)), 0.0);
        assert_eq!(
            ripple.delay(&lanes, 0, &segment(0)),
            ripple.delay(&lanes, 2, &segment(2))
        );
        let pour = MapPaint::new(Vec::new(), None).revealing(MapReveal::Pour, 1);
        assert!(pour.delay(&lanes, 0, &segment(0)) < pour.delay(&lanes, 2, &segment(2)));
    }
}
