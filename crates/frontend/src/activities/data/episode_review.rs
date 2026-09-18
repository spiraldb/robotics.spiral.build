//! Recorded camera feeds and derived evidence on one caller-owned clock.
use super::{
    DatasetClient, episode_list::OpenedEpisode, episode_playback::EpisodeCamera,
    episode_streams::StreamCatalog, episode_transport::EpisodeTransport,
    timeline_panel::TimelinePanel, timeline_tracks::TimelineTrack,
};
use crate::components::interaction::{Action, ActionPurpose, ActionVariant};
use dioxus::prelude::*;

#[component]
pub(crate) fn EpisodeReview(
    episode: OpenedEpisode,
    transport: EpisodeTransport,
    #[props(default)] tracks: Vec<TimelineTrack>,
) -> Element {
    let selection = episode.selection();
    let mut cameras = use_resource(use_reactive(&selection, move |selection| async move {
        DatasetClient::current()
            .episode_topics(selection.clone())
            .await
            .map(|topics| StreamCatalog::new(topics, &selection))
            .map_err(|error| error.to_string())
    }));
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
                                        transport: transport.clone(), show_frame_timing: false,
                                    }
                                }
                            }
                        }
                    }
                },
                Some(Err(error)) => rsx! {
                    div { class: "episode-review-status", role: "alert",
                        p { "Could not read recorded cameras: {error}" }
                        Action { purpose: ActionPurpose::Retry, variant: ActionVariant::Outline,
                            onclick: move |_| cameras.restart(), "Retry cameras" }
                    }
                },
                None => rsx! { p { class: "episode-review-status", role: "status", "Loading recorded cameras…" } },
            }
            TimelinePanel { transport, tracks }
        }
    }
}
