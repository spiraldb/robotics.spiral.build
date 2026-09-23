//! Data's dockable 3D Stage: the open episode's robot, once its streams are
//! known.
use super::stream_choices::EpisodeStreams;
use crate::activities::episode::{Episode, EpisodeTransport, RobotStage, StagePlacement};
use dioxus::prelude::*;

/// The panel an operator opened on purpose, so an episode it cannot draw says
/// why instead of leaving the dock blank.
#[component]
pub(super) fn RobotStagePanel(
    episode: Option<Episode>,
    streams: Option<EpisodeStreams>,
    transport: EpisodeTransport,
) -> Element {
    let Some(episode) = episode else {
        return rsx! { div { class: "data-plots-empty", "Open an episode to see its robot." } };
    };
    let streams = match streams {
        Some(EpisodeStreams::Ready(streams)) => streams,
        Some(EpisodeStreams::Failed(_)) => {
            return rsx! { div { class: "data-plots-empty", role: "status", "3D view unavailable." } };
        }
        _ => return rsx! { div { class: "data-plots-empty", role: "status", "Loading 3D view…" } },
    };
    rsx! {
        RobotStage {
            key: "{episode.dataset().label()}:{episode.id()}",
            episode, streams, transport, placement: StagePlacement::Panel,
        }
    }
}
