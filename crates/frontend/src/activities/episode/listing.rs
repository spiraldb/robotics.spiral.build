//! How an episode is listed: the row it gets in Data's rail and in every
//! Curation index — its still, what it is called, the facts under its name,
//! and its length against the longest listed — worded and drawn the same
//! wherever a list of recordings is read.
use super::identity::{Episode, EpisodeFactsPresentation as _, EpisodeLabels, SourceLabel};
use crate::format::{ClockPresentation as _, WallClockPresentation as _};
use dioxus::prelude::*;

/// What one list of episodes knows about all of them at once: the labels
/// their source paths give, and the longest among them, which every length
/// bar is drawn against.
pub(in crate::activities) struct EpisodeRows {
    labels: EpisodeLabels,
    longest: f64,
}

impl EpisodeRows {
    pub(in crate::activities) fn of<'a>(episodes: impl IntoIterator<Item = &'a Episode>) -> Self {
        let episodes = episodes.into_iter().collect::<Vec<_>>();
        Self {
            labels: EpisodeLabels::of(episodes.iter().map(|episode| episode.facts())),
            longest: episodes
                .iter()
                .filter_map(|episode| episode.facts().duration_seconds())
                .fold(0.0_f64, f64::max),
        }
    }
    /// What `episode`'s source path says about it beside the others.
    pub(in crate::activities) fn label(&self, episode: &Episode) -> SourceLabel {
        self.labels.label(episode.facts())
    }
    /// `episode`'s row: its label when its path gives one, with its id among
    /// the facts, else its id; the day it was recorded when its clock knows;
    /// how long it runs; and its length against the longest listed, on a
    /// square-root scale so one runaway take does not flatten every other bar
    /// into a dot.
    pub(in crate::activities) fn face(&self, episode: &Episode) -> RowFace {
        let facts = episode.facts();
        let id = Episode::listed_name_of(episode.id());
        let (title, mut listed) = match self.label(episode).head() {
            Some(label) => (RowTitle::words(label), vec![id]),
            None => (RowTitle::id(id), Vec::new()),
        };
        if facts
            .message_start_time()
            .is_some_and(|start| start.has_wall_clock())
        {
            listed.push(facts.recorded_date_label());
        }
        listed.extend(
            facts
                .duration_seconds()
                .map(|seconds| seconds.clock_label()),
        );
        let share = facts
            .duration_seconds()
            .filter(|_| self.longest > 0.0)
            .map_or(0.0, |seconds| (seconds / self.longest).sqrt());
        RowFace {
            title,
            facts: listed,
            drawing: Some(RowDrawing::Length(share)),
            measures: Vec::new(),
        }
    }
}

/// What a row is called: words, or an identifier set as the measurement it
/// is.
#[derive(Clone, Debug, PartialEq)]
struct RowTitle {
    words: String,
    is_id: bool,
}

impl RowTitle {
    fn words(words: &str) -> Self {
        Self {
            words: words.to_owned(),
            is_id: false,
        }
    }
    fn id(id: String) -> Self {
        Self {
            words: id,
            is_id: true,
        }
    }
}

/// The small drawing that leads a row's facts: an episode's length against
/// the longest listed, or where a stretch sits in its episode.
#[derive(Clone, Debug, PartialEq)]
enum RowDrawing {
    /// A share of the longest episode listed, 0–1.
    Length(f64),
    /// Where a stretch starts and ends, as fractions of its episode.
    Within(f64, f64),
}

/// A number a row is ranked or kept by, what that number is, and whether it
/// is what leaves the row out.
#[derive(Clone, Debug, PartialEq)]
struct RowMeasure {
    reading: String,
    meaning: String,
    failing: bool,
}

/// One row of a list of recordings, in the anatomy every such row shares: a
/// still, a name over a line of facts, and its measures, one over another.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::activities) struct RowFace {
    title: RowTitle,
    facts: Vec<String>,
    drawing: Option<RowDrawing>,
    measures: Vec<RowMeasure>,
}

impl RowFace {
    /// A stretch of one episode, listed under its episode's heading: named by
    /// its span, saying how long it runs — a stretch too short to train on
    /// reads as one — and drawn where it sits in the episode's `duration`.
    pub(in crate::activities) fn span((start, end): (f64, f64), duration: f64) -> Self {
        let duration = duration.max(f64::EPSILON);
        Self {
            title: RowTitle::id(format!("{} – {}", start.clock_label(), end.clock_label())),
            facts: vec![format!("{:.1} s", (end - start).max(0.0))],
            drawing: Some(RowDrawing::Within(start / duration, end / duration)),
            measures: Vec::new(),
        }
    }
    /// Replace the facts line's words, keeping its drawing.
    pub(in crate::activities) fn saying(mut self, facts: Vec<String>) -> Self {
        self.facts = facts;
        self
    }
    /// One more measure, under the ones before it.
    pub(in crate::activities) fn measured(
        self,
        reading: String,
        meaning: impl Into<String>,
    ) -> Self {
        self.judged(reading, meaning, false)
    }
    /// One more measure, `failing` when it is what leaves the row out of a
    /// rule.
    pub(in crate::activities) fn judged(
        mut self,
        reading: String,
        meaning: impl Into<String>,
        failing: bool,
    ) -> Self {
        self.measures.push(RowMeasure {
            reading,
            meaning: meaning.into(),
            failing,
        });
        self
    }
}

/// A row's facts-line drawing.
#[component]
fn RowDrawingView(drawing: RowDrawing) -> Element {
    let style = match drawing {
        RowDrawing::Length(share) => format!("width: {:.1}%", share.clamp(0.0, 1.0) * 100.0),
        RowDrawing::Within(left, right) => {
            let (left, right) = (left.clamp(0.0, 1.0), right.clamp(0.0, 1.0));
            format!(
                "left: {:.2}%; width: {:.2}%",
                left * 100.0,
                (right - left).max(0.0) * 100.0
            )
        }
    };
    rsx! {
        span { class: "episode-row-extent", "aria-hidden": "true", i { style } }
    }
}

/// What a row shows, inside whatever makes it pressable — a tree item in
/// Data's rail, an index row in Curation: `still`, the name over its facts,
/// and the measure.
#[component]
pub(in crate::activities) fn RowFaceView(face: RowFace, still: Element) -> Element {
    rsx! {
        {still}
        span { class: "episode-row-words",
            span { class: "episode-row-title", "data-id": face.title.is_id.then_some("true"),
                "{face.title.words}"
            }
            // The row is read aloud as one name; the separators the eye gets
            // from the layout, the ear gets as commas.
            span { class: "episode-row-facts",
                if let Some(drawing) = face.drawing.clone() {
                    RowDrawingView { drawing }
                }
                for fact in face.facts.iter() {
                    span { class: "ui-sr-only", ", " }
                    span { "{fact}" }
                }
            }
        }
        if !face.measures.is_empty() {
            span { class: "episode-row-measures",
                for measure in face.measures.iter() {
                    span {
                        class: "episode-row-measure",
                        "data-failing": measure.failing.then_some("true"),
                        title: "{measure.meaning}",
                        "{measure.reading}"
                    }
                }
            }
        }
    }
}
