//! Recorded camera feeds and derived evidence on one caller-owned clock.
//! Shared by Curation's evidence panels and Build's subtask views.
use super::{DatasetClient, EpisodeCamera, EpisodeTransport, OpenedEpisode, StreamCatalog};
use crate::components::timeline::{
    IntervalTrack, IntervalTrackRow, NumericTrackRow, Timeline, TimelineAction, TimelineContent,
    TimelineEvent, TimelineInteraction, TimelineInterval, TimelineTrack,
};
use dioxus::prelude::*;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant};

#[component]
pub(in crate::activities) fn EpisodeReview(
    episode: OpenedEpisode,
    transport: EpisodeTransport,
    #[props(default)] tracks: Vec<TimelineTrack>,
    /// Rows of named spans drawn below the numeric tracks, such as a
    /// measurement's subtask intervals. Activating one reaches `on_interval`.
    #[props(default)]
    span_tracks: Vec<IntervalTrack>,
    #[props(default)] on_interval: EventHandler<TimelineInterval>,
) -> Element {
    let selection = episode.selection();
    let mut cameras = use_resource(use_reactive(&selection, move |selection| async move {
        DatasetClient::current()
            .episode_topics(selection.clone())
            .await
            .map(|topics| StreamCatalog::new(topics, &selection))
            .map_err(|error| error.to_string())
    }));
    let default_interaction = transport.interaction();
    let current = transport.clone();
    let interaction = TimelineInteraction::new(
        transport.timeline_state(),
        EventHandler::new(move |event: TimelineEvent| {
            if event.target() != current.timeline_state().target() {
                return;
            }
            match event.action() {
                TimelineAction::ActivateInterval(interval) => on_interval.call(interval),
                action => default_interaction.emit(action),
            }
        }),
    );
    let rows = rsx! {
        for track in tracks { NumericTrackRow { key: "{track.id()}", track, interaction: interaction.clone() } }
        for track in span_tracks { IntervalTrackRow { key: "{track.id()}", track, interaction: interaction.clone() } }
    };
    let content = TimelineContent::new(rows).with_identity(rsx! { strong { "{episode.title()}" } });
    rsx! {
        section { class: "episode-review", "aria-label": "Recorded episode review",
            match cameras.read().as_ref() {
                Some(Ok(catalog)) => rsx! {
                    if catalog.videos().next().is_none() {
                        p { class: "episode-review-status", "This episode has no recorded cameras." }
                    } else {
                        div { class: "episode-review-cameras",
                            for camera in catalog.videos() {
                                section { key: "{camera.key()}", class: "episode-review-feed",
                                    h5 { title: camera.label(), "{camera.label().trim_start_matches('/')}" }
                                    EpisodeCamera {
                                        request: camera.read().clone(), timeline: transport.range(),
                                        transport: transport.clone(),
                                    }
                                }
                            }
                        }
                    }
                },
                Some(Err(error)) => rsx! {
                    div { class: "episode-review-status", role: "alert",
                        p { "Could not read recorded cameras: {error}" }
                        Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline,
                            onclick: move |_| cameras.restart(), "Retry cameras" }
                    }
                },
                None => rsx! { p { class: "episode-review-status", role: "status", "Loading recorded cameras…" } },
            }
            Timeline { interaction, content }
        }
    }
}
