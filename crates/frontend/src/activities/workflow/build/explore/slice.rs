//! One subtask row as both explorations show it: a light card, and the
//! recorded camera only for the card the operator opened.
//!
//! A result is a video slice, but every playing slice decodes its own
//! episode, so a grid of cards that all played would open a decoder per
//! result. The card carries text alone; selecting one mounts the shared
//! episode player and plays that interval.
use crate::activities::data::{DatasetClient, EpisodeReview, EpisodeTransport, OpenedEpisode};
use crate::activities::workflow::build::document::SubtaskRow;
use crate::format::ClockPresentation as _;
use dioxus::prelude::*;
use spiral_robotics_common::datasets::{DatasetSelection, EpisodeSelection};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonSize, ButtonVariant, Notice, NoticeTone};

/// What jev said about a row, once it has been asked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SliceScore {
    probability: f64,
    answered: bool,
}
impl SliceScore {
    pub(super) fn new(probability: f64, answered: bool) -> Self {
        Self {
            probability,
            answered,
        }
    }
    fn label(self) -> String {
        if self.answered {
            format!("{:.1}%", self.probability * 100.0)
        } else {
            "no answer".to_owned()
        }
    }
}

/// What a card can do: open its camera in place, or send the operator to
/// this interval in the annotation view it came from.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct SliceActions {
    on_select: EventHandler<()>,
    on_open: EventHandler<()>,
}
impl SliceActions {
    pub(super) fn new(on_select: EventHandler<()>, on_open: EventHandler<()>) -> Self {
        Self { on_select, on_open }
    }
}

/// One result: its probability, description, episode, and interval, with
/// the camera it was given as children once it is the selected card.
#[component]
pub(super) fn SubtaskSlice(
    row: SubtaskRow,
    score: Option<SliceScore>,
    selected: bool,
    actions: SliceActions,
    children: Element,
) -> Element {
    let interval = format!(
        "{} – {}",
        row.start_time().clock_label(),
        row.end_time().clock_label()
    );
    rsx! {
        article { class: "build-explore-slice", "data-selected": "{selected}",
            button {
                class: "build-explore-slice-head",
                "aria-pressed": "{selected}",
                onclick: move |_| actions.on_select.call(()),
                if let Some(score) = score {
                    span {
                        class: "build-explore-slice-score",
                        "data-answered": "{score.answered}",
                        "{score.label()}"
                    }
                }
                span { class: "build-explore-slice-description", "{row.description()}" }
            }
            div { class: "build-explore-slice-facts",
                span { title: "{row.episode()}", "Episode {row.episode().chars().take(8).collect::<String>()}" }
                span { "{interval}" }
                Button {
                    purpose: ActionPurpose::Navigate,
                    variant: ButtonVariant::Link,
                    size: ButtonSize::Compact,
                    onclick: move |_| actions.on_open.call(()),
                    "Open in context"
                }
            }
            {children}
        }
    }
}

/// The recorded cameras of one slice, played over its interval alone.
#[component]
pub(super) fn SubtaskCamera(
    dataset: DatasetSelection,
    episode: String,
    interval: (f64, f64),
) -> Element {
    let opened = use_resource(use_reactive(
        (&dataset, &episode),
        move |(dataset, episode)| async move { SliceEpisode::open(dataset, episode).await },
    ));
    match opened.read().as_ref() {
        Some(Ok(episode)) => rsx! { SlicePlayer { episode: episode.clone(), interval } },
        Some(Err(error)) => rsx! {
            Notice { tone: NoticeTone::Danger, "Could not open this episode: {error}" }
        },
        None => rsx! {
            p { class: "build-explore-note", role: "status", "Opening the episode…" }
        },
    }
}

#[component]
fn SlicePlayer(episode: OpenedEpisode, interval: (f64, f64)) -> Element {
    let transport = EpisodeTransport::use_opened_episode(&episode);
    let mut playing = transport.clone();
    use_effect(use_reactive(&interval, move |(start, end)| {
        playing.play_interval(start, end);
    }));
    rsx! {
        div { class: "build-explore-slice-camera", EpisodeReview { episode, transport } }
    }
}

/// The episode identity the shared player needs, read for one episode.
struct SliceEpisode;
impl SliceEpisode {
    async fn open(dataset: DatasetSelection, episode: String) -> Result<OpenedEpisode, String> {
        let request = EpisodeQueryRequest::new(
            EpisodeSelection::new(dataset.clone(), episode),
            Self::sql(&dataset),
            1,
        );
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
    /// `$1` is the episode the request names; the columns are the ones the
    /// Data rail reads, so the player sees the episode it always sees.
    fn sql(dataset: &DatasetSelection) -> String {
        format!(
            "SELECT id, source.uri AS source_uri, source.size AS source_bytes, message_count, \
             message_start_time, message_end_time, duration_ns, channel_count \
             FROM spql.mcap.episodes('{}') WHERE id = $1",
            dataset.sql_name().replace('\'', "''")
        )
    }
}
