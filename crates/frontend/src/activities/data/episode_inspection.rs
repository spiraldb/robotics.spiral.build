//! One recording seen whole: every camera it recorded, the robot when the
//! dataset has a model, and derived evidence, all on one caller-owned clock.
//!
//! This is the episode view the whole studio shares — Curation's evidence
//! panels, Workflow's subtask views — so an operator recognizes the same
//! recording wherever it is shown. Callers own the clock and may narrow the
//! cameras; everything else the episode itself decides.
use super::{
    DatasetClient, EpisodeCamera, EpisodeTransport, OpenedEpisode, StreamCatalog,
    robot_stage::RobotStage,
};
use crate::components::timeline::{
    IntervalTrack, IntervalTrackRow, NumericTrackRow, Timeline, TimelineAction, TimelineContent,
    TimelineEvent, TimelineInteraction, TimelineInterval, TimelineTrack,
};
use dioxus::prelude::*;
use spiral_robotics_ui::{ActionPurpose, Button, ButtonVariant};

/// Which of an episode's cameras takes the height in this view.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(in crate::activities) enum FeaturedCamera {
    /// The episode's first camera, in its own topic order.
    #[default]
    First,
    /// The camera this view is about, such as the one a measurement read. An
    /// episode that did not record it leads with its first.
    Named(String),
    /// Whichever camera shows the workspace best, for a view that is about the
    /// recording rather than about one camera.
    Overview,
}

impl FeaturedCamera {
    /// Where in `shown` the featured camera sits.
    fn leading(&self, shown: &[&super::episode_streams::RecordedStream]) -> usize {
        let wanted = match self {
            Self::First => None,
            Self::Named(camera) => Some(camera.clone()),
            Self::Overview => {
                let names = shown
                    .iter()
                    .map(|camera| camera.name().to_owned())
                    .collect::<Vec<_>>();
                StreamCatalog::overview_camera(&names).map(str::to_owned)
            }
        };
        wanted
            .and_then(|wanted| shown.iter().position(|camera| camera.name() == wanted))
            .unwrap_or_default()
    }
}

#[component]
pub(in crate::activities) fn EpisodeInspection(
    episode: OpenedEpisode,
    transport: EpisodeTransport,
    #[props(default)] tracks: Vec<TimelineTrack>,
    /// Rows of named spans drawn below the numeric tracks, such as a
    /// measurement's subtask intervals. Activating one reaches `on_interval`.
    #[props(default)]
    span_tracks: Vec<IntervalTrack>,
    #[props(default)] on_interval: EventHandler<TimelineInterval>,
    /// The camera topics worth opening here, when the caller only means some
    /// of them: a measurement that read one camera shows that camera, so the
    /// view never decodes streams the operator is not being asked about.
    /// Empty opens every camera the episode recorded.
    #[props(default)]
    shown_cameras: Vec<String>,
    /// Which camera takes the height while the rest sit under it.
    #[props(default)]
    featured_camera: FeaturedCamera,
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
    let numeric = !tracks.is_empty();
    let trackless = tracks.is_empty() && span_tracks.is_empty();
    let rows = rsx! {
        for track in tracks { NumericTrackRow { key: "{track.id()}", track, interaction: interaction.clone() } }
        for track in span_tracks { IntervalTrackRow { key: "{track.id()}", track, interaction: interaction.clone() } }
    };
    // Every surface that mounts this composition names the episode directly
    // above it, so the transport's identity slot would only repeat it — as a
    // raw identifier, pressed against the transport. "Tracks" goes with it
    // when there are no numeric rows for it to head.
    let content = TimelineContent::new(rows).with_identity(rsx! {});
    let content = if numeric {
        content
    } else {
        content.with_corner(rsx! {})
    };
    rsx! {
        section {
            class: "episode-inspection",
            "aria-label": "Recorded episode",
            "data-tracks": trackless.then_some("none"),
            match cameras.read().as_ref() {
                Some(Ok(catalog)) => {
                    let shown = catalog.shown(&shown_cameras).collect::<Vec<_>>();
                    let lead = featured_camera.leading(&shown);
                    let rest = shown
                        .iter()
                        .enumerate()
                        .filter(|(place, _)| *place != lead)
                        .map(|(_, camera)| *camera)
                        .collect::<Vec<_>>();
                    rsx! {
                        if shown.is_empty() {
                            p { class: "episode-inspection-status",
                                if shown_cameras.is_empty() { "This episode has no recorded cameras." }
                                else { "This episode did not record {shown_cameras.join(\", \")}." }
                            }
                        }
                        div { class: "episode-inspection-tiles",
                            if let Some(camera) = shown.get(lead) {
                                section { class: "episode-inspection-tile episode-inspection-featured",
                                    h5 { title: camera.label(), "{catalog.camera_title(camera)}" }
                                    EpisodeCamera {
                                        request: camera.read().clone(), timeline: transport.range(),
                                        transport: transport.clone(),
                                    }
                                }
                            }
                            div { class: "episode-inspection-strip",
                                for camera in rest {
                                    section { key: "{camera.key()}", class: "episode-inspection-tile",
                                        h5 { title: camera.label(), "{catalog.camera_title(camera)}" }
                                        EpisodeCamera {
                                            request: camera.read().clone(), timeline: transport.range(),
                                            transport: transport.clone(),
                                        }
                                    }
                                }
                                RobotStage {
                                    episode: episode.clone(), streams: catalog.clone(),
                                    transport: transport.clone(),
                                }
                            }
                        }
                    }
                },
                Some(Err(error)) => rsx! {
                    div { class: "episode-inspection-status", role: "alert",
                        p { "Could not read recorded cameras: {error}" }
                        Button { purpose: ActionPurpose::Retry, variant: ButtonVariant::Outline,
                            onclick: move |_| cameras.restart(), "Retry cameras" }
                    }
                },
                None => rsx! { p { class: "episode-inspection-status", role: "status", "Loading recorded cameras…" } },
            }
            Timeline { interaction, content }
        }
    }
}
