//! How every Build measurement's output is read: an index that scrolls beside
//! a recording that holds still.
//!
//! All three output views are the same thing — a list of subtask intervals,
//! and one of them shown as video. Written separately they each pushed the
//! recording off the bottom of a growing list, so the click that plays a
//! subtask had nothing visible to show for itself. One frame owns that
//! proportion now: the index owns the scroll, the evidence column never
//! moves, and the row the operator chose is always beside its own footage.
use super::document::SubtaskRow;
use super::poster::{PosterFrame, PosterFrames};
use crate::activities::data::{DatasetClient, EpisodeReview, EpisodeTransport, OpenedEpisode};
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeSelection};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use spiral_robotics_ui::{
    ActionPurpose, Button, ButtonSize, ButtonVariant, EmptyState, Notice, NoticeTone,
};

/// A subtask plays with a little lead-in and run-out, so the operator sees the
/// motion enter and leave the interval rather than starting mid-reach.
pub(super) const INTERVAL_MARGIN_SECONDS: f64 = 0.5;

/// Episode numbers as the operator counts them.
///
/// A UUID is not a name. Build lists recordings in recording order everywhere,
/// so a row reads "Episode 7" and keeps its identifier in the tooltip for the
/// one moment somebody needs to paste it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct EpisodeNames(Vec<String>);

impl EpisodeNames {
    /// The episodes in the order they were measured, keeping the first sight
    /// of each so repeated rows of one episode all read the same number.
    pub(super) fn of<'a>(episodes: impl Iterator<Item = &'a str>) -> Self {
        let mut ordered = Vec::<String>::new();
        for episode in episodes {
            if !ordered.iter().any(|seen| seen == episode) {
                ordered.push(episode.to_owned());
            }
        }
        Self(ordered)
    }
    /// What to call one episode. An episode this index never saw keeps the
    /// *tail* of its identifier rather than a number that would mean nothing:
    /// these are time-ordered ULIDs, so every recording of one dataset shares
    /// its head and only the tail tells them apart.
    pub(super) fn name(&self, episode: &str) -> String {
        match self.0.iter().position(|seen| seen == episode) {
            Some(index) => format!("Episode {}", index + 1),
            None => format!(
                "…{}",
                episode
                    .chars()
                    .rev()
                    .take(6)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>()
            ),
        }
    }
}

/// What a view needs to show any row of a measurement: where the recordings
/// live, which camera the measurement read, and what the episodes are called.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct SubtaskContext {
    dataset: Option<DatasetSelection>,
    camera: Option<String>,
    names: EpisodeNames,
}

impl SubtaskContext {
    pub(super) fn new(
        dataset: Option<DatasetSelection>,
        camera: Option<String>,
        names: EpisodeNames,
    ) -> Self {
        Self {
            dataset,
            camera,
            names,
        }
    }
    pub(super) fn dataset(&self) -> Option<&DatasetSelection> {
        self.dataset.as_ref()
    }
    pub(super) fn camera(&self) -> Option<&str> {
        self.camera.as_deref()
    }
    pub(super) fn name(&self, episode: &str) -> String {
        self.names.name(episode)
    }
    /// The cameras `EpisodeReview` opens here: the measured one alone, or
    /// every recorded camera when the measurement never named one.
    fn shown_cameras(&self) -> Vec<String> {
        self.camera.clone().into_iter().collect()
    }
}

/// What an index says about one row beyond its own words: what jev made of
/// it, and whether it is the row the evidence column is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct SubtaskStanding {
    /// `None` before anything has been asked about this row.
    probability: Option<f64>,
    /// A batch that came back without an answer for this row leaves it in the
    /// list, last and unscored, rather than dropping it silently.
    answered: bool,
    chosen: bool,
}

impl SubtaskStanding {
    /// A row nobody has scored: the annotation's own output.
    pub(super) fn plain(chosen: bool) -> Self {
        Self {
            probability: None,
            answered: true,
            chosen,
        }
    }
    pub(super) fn scored(probability: f64, answered: bool, chosen: bool) -> Self {
        Self {
            probability: Some(probability),
            answered,
            chosen,
        }
    }
    /// What the score column reads. One decimal is as much as a calibrated
    /// probability earns.
    fn label(self) -> Option<String> {
        let probability = self.probability?;
        Some(if self.answered {
            format!("{:.1}%", probability * 100.0)
        } else {
            "—".to_owned()
        })
    }
}

/// One row of an index: its poster, what jev made of it, its words, and where
/// in the dataset it happened.
///
/// The same row in all three views, because it is the same thing in all three.
#[component]
pub(super) fn SubtaskEntry(
    row: SubtaskRow,
    standing: SubtaskStanding,
    /// What to call the episode, for an index that is not already grouped by
    /// one. Grouped indexes leave it out rather than repeat their heading.
    episode: Option<String>,
    frames: PosterFrames,
    on_select: EventHandler<()>,
) -> Element {
    let span = format!(
        "{} – {}",
        row.start_time().clock_label(),
        row.end_time().clock_label()
    );
    rsx! {
        li { class: "build-entry-item",
            button {
                r#type: "button",
                class: "build-entry",
                "aria-pressed": "{standing.chosen}",
                title: "{row.description()}",
                onclick: move |_| on_select.call(()),
                PosterFrame { row: row.clone(), frames }
                span { class: "build-entry-words",
                    span { class: "build-entry-description", "{row.description()}" }
                    span { class: "build-entry-facts",
                        if let Some(episode) = episode.clone() {
                            span { title: "{row.episode()}", "{episode}" }
                        }
                        span { "{span}" }
                    }
                }
                if let Some(score) = standing.label() {
                    span {
                        class: "build-entry-score",
                        "data-answered": "{standing.answered}",
                        title: if standing.answered { "How likely jev thinks this is the same operation" }
                               else { "This row came back without an answer" },
                        "{score}"
                    }
                }
            }
        }
    }
}

/// The index beside the recording. The caller fills both halves; this owns
/// only which one scrolls.
#[component]
pub(super) fn SubtaskReview(index: Element, evidence: Element) -> Element {
    rsx! {
        div { class: "build-review",
            div { class: "build-review-index", {index} }
            aside { class: "build-review-evidence", "aria-label": "Selected subtask", {evidence} }
        }
    }
}

/// One subtask row shown as what it is: the camera the measurement read,
/// played over that row's interval alone.
///
/// The measurement sampled one camera, so this opens one camera. Mounting the
/// episode's whole camera set here asked Spiral for three full recordings in
/// order to show six seconds of one of them, and two of the three came back
/// over the message-page limit.
#[component]
pub(super) fn SubtaskEvidence(
    row: SubtaskRow,
    context: SubtaskContext,
    /// Send the operator to this interval in the annotation it came from.
    on_open: EventHandler<()>,
) -> Element {
    let Some(dataset) = context.dataset().cloned() else {
        return rsx! {
            Notice { tone: NoticeTone::Caution,
                "This build has no dataset, so the recording cannot be opened."
            }
        };
    };
    let episode = row.episode().to_owned();
    let opened = use_resource(use_reactive(
        (&dataset, &episode),
        move |(dataset, episode)| async move { open_episode(dataset, episode).await },
    ));
    let span = (row.start_time(), row.end_time());
    rsx! {
        header { class: "build-review-head",
            p { class: "build-review-subject", "{row.description()}" }
            p { class: "build-review-facts",
                span { title: "{row.episode()}", "{context.name(row.episode())}" }
                span { "{span.0.clock_label()} – {span.1.clock_label()}" }
                Button {
                    purpose: ActionPurpose::Navigate,
                    variant: ButtonVariant::Link,
                    size: ButtonSize::Compact,
                    title: "Show this interval in the annotation it came from",
                    onclick: move |_| on_open.call(()),
                    "Open in context"
                }
            }
        }
        match opened.read().as_ref() {
            Some(Ok(episode)) => rsx! {
                SlicePlayer {
                    key: "{episode.title()}",
                    episode: episode.clone(),
                    span,
                    cameras: context.shown_cameras(),
                }
            },
            Some(Err(error)) => rsx! {
                Notice { tone: NoticeTone::Danger, "Could not open this episode: {error}" }
            },
            None => rsx! {
                p { class: "build-review-note", role: "status", "Opening the recording…" }
            },
        }
    }
}

/// The recording, seeking to the interval whenever the chosen row changes.
#[component]
fn SlicePlayer(episode: OpenedEpisode, span: (f64, f64), cameras: Vec<String>) -> Element {
    let transport = EpisodeTransport::use_opened_episode(&episode);
    let mut playing = transport.clone();
    use_effect(use_reactive(&span, move |(start, end)| {
        playing.play_interval(
            (start - INTERVAL_MARGIN_SECONDS).max(0.0),
            end + INTERVAL_MARGIN_SECONDS,
        );
    }));
    rsx! {
        div { class: "build-review-player",
            EpisodeReview { episode, transport, shown_cameras: cameras }
        }
    }
}

/// Nothing chosen yet: say what choosing one does rather than leaving the
/// column blank beside a full list.
#[component]
pub(super) fn NoSubtaskChosen(prompt: String) -> Element {
    rsx! {
        EmptyState { title: "Nothing selected", p { "{prompt}" } }
    }
}

/// The episode identity the shared player needs, read for one episode.
async fn open_episode(dataset: DatasetSelection, episode: String) -> Result<OpenedEpisode, String> {
    let sql = format!(
        "SELECT id, source.uri AS source_uri, source.size AS source_bytes, message_count, \
         message_start_time, message_end_time, duration_ns, channel_count \
         FROM spql.mcap.episodes('{}') WHERE id = $1",
        dataset.sql_name().replace('\'', "''")
    );
    let request = EpisodeQueryRequest::new(EpisodeSelection::new(dataset.clone(), episode), sql, 1);
    let row = DatasetClient::current()
        .run_episode_query(&request)
        .await
        .map_err(|error| error.to_string())?
        .into_rows()
        .into_iter()
        .next()
        .ok_or("This subtask's episode is no longer in the dataset.")?;
    OpenedEpisode::from_row(dataset, row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episodes_are_numbered_in_the_order_they_were_measured() {
        let names = EpisodeNames::of(["ep-a", "ep-a", "ep-b", "ep-a"].into_iter());
        assert_eq!(names.name("ep-a"), "Episode 1");
        assert_eq!(names.name("ep-b"), "Episode 2");
    }

    #[test]
    fn an_episode_outside_the_index_keeps_the_tail_of_its_identifier() {
        // Every recording of one dataset shares the head of its ULID, so the
        // head names nothing; the tail is what differs.
        let names = EpisodeNames::of(["ep-a"].into_iter());
        assert_eq!(names.name("01a0989fb65870a7"), "…5870a7");
    }
}
